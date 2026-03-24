use crate::model::GeoPoint;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use super::db::runtime_db_path;
use super::util::{
    bounds_intersect, canonical_road_class, expand_bounds, parse_geojson_linestring,
    parse_geojson_multilinestring, polyline_bounds, road_class_matches,
};
use super::{
    FOCUS_NODE_MARGIN_DEGREES, GeoBounds, OsmFeatureKind, RoadLayerKind, RoadPolyline,
};
use osmpbf::{Element, ElementReader};

pub(super) fn vector_cache_dir(db_path: &Path) -> Result<PathBuf, String> {
    let dir = db_path
        .parent()
        .ok_or("OSM runtime DB has no parent directory")?
        .join("road_cells");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

pub(super) fn vector_cell_path(cache_dir: &Path, cell_lat: i32, cell_lon: i32) -> PathBuf {
    cache_dir.join(format!("road_cell_{:+04}_{:+05}.geojson", cell_lat, cell_lon))
}

pub(super) fn ensure_cell_geojson_from_extract(
    extract_path: &Path,
    output_path: &Path,
    bounds: GeoBounds,
) -> Result<usize, String> {
    if output_path.exists() {
        return Ok(0);
    }

    let expanded_bounds = expand_bounds(bounds, FOCUS_NODE_MARGIN_DEGREES);
    let reader = ElementReader::from_path(extract_path).map_err(|error| {
        format!(
            "Failed to open focused road extract {}: {error}",
            extract_path.display()
        )
    })?;

    let mut candidate_nodes: HashMap<i64, GeoPoint> = HashMap::new();
    let mut seen_way_ids = HashSet::new();
    let mut features = Vec::new();

    reader
        .for_each(|element| match element {
            Element::Node(node) => {
                let point = GeoPoint {
                    lat: node.lat() as f32,
                    lon: node.lon() as f32,
                };
                if super::util::point_in_bounds(point, expanded_bounds) {
                    candidate_nodes.insert(node.id(), point);
                }
            }
            Element::DenseNode(node) => {
                let point = GeoPoint {
                    lat: node.lat() as f32,
                    lon: node.lon() as f32,
                };
                if super::util::point_in_bounds(point, expanded_bounds) {
                    candidate_nodes.insert(node.id(), point);
                }
            }
            Element::Way(way) => {
                let mut highway_class = None;
                let mut road_name = None;
                for (key, value) in way.tags() {
                    if key == "highway" {
                        highway_class = canonical_road_class(value);
                    } else if key == "name" && road_name.is_none() {
                        road_name = Some(value.to_owned());
                    }
                }
                let Some(road_class) = highway_class else {
                    return;
                };
                if !seen_way_ids.insert(way.id()) {
                    return;
                }

                let points: Vec<_> = way
                    .refs()
                    .filter_map(|node_id| candidate_nodes.get(&node_id).copied())
                    .collect();
                if points.len() < 2 {
                    return;
                }

                let way_bounds = polyline_bounds(&points);
                if !bounds_intersect(way_bounds, bounds) {
                    return;
                }

                let coordinates: Vec<Value> = points
                    .iter()
                    .map(|point| json!([point.lon as f64, point.lat as f64]))
                    .collect();
                features.push(json!({
                    "type": "Feature",
                    "properties": {
                        "way_id": way.id(),
                        "class": road_class,
                        "name": road_name,
                    },
                    "geometry": {
                        "type": "LineString",
                        "coordinates": coordinates,
                    }
                }));
            }
            Element::Relation(_) => {}
        })
        .map_err(|error| error.to_string())?;

    let feature_count = features.len();
    let body = serde_json::to_string(&json!({
        "type": "FeatureCollection",
        "features": features,
    }))
    .map_err(|error| error.to_string())?;
    fs::write(output_path, body).map_err(|error| error.to_string())?;
    Ok(feature_count)
}

pub fn load_roads_for_bounds_from_vector_cache(
    selected_root: Option<&Path>,
    bounds: GeoBounds,
    layer_kind: RoadLayerKind,
) -> Option<Vec<RoadPolyline>> {
    let db_path = runtime_db_path(selected_root)?;
    let cache_dir = db_path.parent()?.join("road_cells");
    if !cache_dir.exists() {
        return None;
    }

    let cells = focus_cells_for_bounds(bounds);
    let mut any_file = false;
    let mut seen_way_ids = HashSet::new();
    let mut roads = Vec::new();

    for (cell_lat, cell_lon) in cells {
        let path = vector_cell_path(&cache_dir, cell_lat, cell_lon);
        if !path.exists() {
            continue;
        }
        any_file = true;
        let Ok(body) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(payload) = serde_json::from_str::<Value>(&body) else {
            continue;
        };
        let Some(features) = payload.get("features").and_then(Value::as_array) else {
            continue;
        };

        for feature in features {
            let props = feature.get("properties").unwrap_or(&Value::Null);
            let way_id = props
                .get("way_id")
                .and_then(Value::as_i64)
                .unwrap_or_default();
            let road_class = props
                .get("class")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            if !road_class_matches(&road_class, layer_kind) || !seen_way_ids.insert(way_id) {
                continue;
            }

            let Some(geometry) = feature.get("geometry") else {
                continue;
            };
            let points = match geometry.get("type").and_then(Value::as_str) {
                Some("LineString") => parse_geojson_linestring(geometry),
                Some("MultiLineString") => parse_geojson_multilinestring(geometry)
                    .and_then(|mut lines| lines.drain(..).next()),
                _ => None,
            };
            let Some(points) = points else {
                continue;
            };
            if points.len() < 2 {
                continue;
            }
            let road_bounds = polyline_bounds(&points);
            if !bounds_intersect(road_bounds, bounds) {
                continue;
            }

            roads.push(RoadPolyline {
                way_id,
                road_class,
                name: props
                    .get("name")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
                    .filter(|name| !name.is_empty()),
                points,
            });
        }
    }

    if any_file { Some(roads) } else { None }
}

pub(super) fn write_roads_to_vector_cells(
    db_path: &Path,
    bounds: GeoBounds,
    roads: &[RoadPolyline],
) -> Result<usize, String> {
    let cache_dir = vector_cache_dir(db_path)?;
    let cells = focus_cells_for_bounds(bounds);
    let mut written_cells = 0usize;

    for (cell_lat, cell_lon) in cells {
        let cell_bounds = GeoBounds {
            min_lat: cell_lat as f32,
            max_lat: (cell_lat + 1) as f32,
            min_lon: cell_lon as f32,
            max_lon: (cell_lon + 1) as f32,
        };
        let path = vector_cell_path(&cache_dir, cell_lat, cell_lon);
        let mut merged: HashMap<i64, RoadPolyline> = load_all_roads_from_vector_cell(&path)
            .unwrap_or_default()
            .into_iter()
            .map(|road| (road.way_id, road))
            .collect();

        let mut changed = false;
        for road in roads {
            if !bounds_intersect(polyline_bounds(&road.points), cell_bounds) {
                continue;
            }
            if merged.insert(road.way_id, road.clone()).is_none() {
                changed = true;
            }
        }

        if !changed && path.exists() {
            continue;
        }

        let mut features = Vec::with_capacity(merged.len());
        for road in merged.values() {
            let coordinates: Vec<Value> = road
                .points
                .iter()
                .map(|point| json!([point.lon as f64, point.lat as f64]))
                .collect();
            features.push(json!({
                "type": "Feature",
                "properties": {
                    "way_id": road.way_id,
                    "class": road.road_class,
                    "name": road.name,
                },
                "geometry": {
                    "type": "LineString",
                    "coordinates": coordinates,
                }
            }));
        }

        let body = serde_json::to_string(&json!({
            "type": "FeatureCollection",
            "features": features,
        }))
        .map_err(|error| error.to_string())?;
        fs::write(&path, body).map_err(|error| error.to_string())?;
        written_cells += 1;
    }

    Ok(written_cells)
}

fn focus_cells_for_bounds(bounds: GeoBounds) -> Vec<(i32, i32)> {
    let min_lat_c = bounds.min_lat.floor() as i32;
    let max_lat_c = bounds.max_lat.floor() as i32;
    let min_lon_c = bounds.min_lon.floor() as i32;
    let max_lon_c = bounds.max_lon.floor() as i32;
    (min_lat_c..=max_lat_c)
        .flat_map(|lat| (min_lon_c..=max_lon_c).map(move |lon| (lat, lon)))
        .collect()
}

fn load_all_roads_from_vector_cell(path: &Path) -> Option<Vec<RoadPolyline>> {
    let body = fs::read_to_string(path).ok()?;
    let payload = serde_json::from_str::<Value>(&body).ok()?;
    let features = payload.get("features").and_then(Value::as_array)?;
    let mut roads = Vec::new();

    for feature in features {
        let props = feature.get("properties").unwrap_or(&Value::Null);
        let way_id = props.get("way_id").and_then(Value::as_i64).unwrap_or_default();
        let road_class = props
            .get("class")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let geometry = feature.get("geometry")?;
        let points = match geometry.get("type").and_then(Value::as_str) {
            Some("LineString") => parse_geojson_linestring(geometry),
            Some("MultiLineString") => parse_geojson_multilinestring(geometry)
                .and_then(|mut lines| lines.drain(..).next()),
            _ => None,
        }?;
        if points.len() < 2 {
            continue;
        }
        roads.push(RoadPolyline {
            way_id,
            road_class,
            name: props
                .get("name")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
                .filter(|name| !name.is_empty()),
            points,
        });
    }

    Some(roads)
}
