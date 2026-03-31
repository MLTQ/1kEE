use crate::model::GeoPoint;
use cell_format::{
    CellFeature, CellPoint, TAG_ROAD, cell_filename, decode_road_class, encode_road_class,
    read::read_single_chunk, write::write_cell,
};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use super::db::runtime_db_path;
use super::util::{
    bounds_intersect, canonical_road_class, expand_bounds, parse_geojson_linestring,
    parse_geojson_multilinestring, polyline_bounds, road_class_matches,
};
use super::{FOCUS_NODE_MARGIN_DEGREES, GeoBounds, RoadLayerKind, RoadPolyline};
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
    cache_dir.join(cell_filename("road", cell_lat, cell_lon))
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
    let mut cell_features: Vec<CellFeature> = Vec::new();

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

                cell_features.push(CellFeature {
                    way_id: way.id(),
                    class: encode_road_class(road_class),
                    is_polygon: false,
                    name: road_name,
                    points: points
                        .into_iter()
                        .map(|p| CellPoint {
                            lon: p.lon,
                            lat: p.lat,
                        })
                        .collect(),
                    elevations: None,
                });
            }
            Element::Relation(_) => {}
        })
        .map_err(|error| error.to_string())?;

    let feature_count = cell_features.len();

    // Derive cell coordinates from the output path stem.
    // Path is …/road_cells/road_cell_{lat}_{lon}.1kc; we use (0,0) as a safe
    // fallback since the cell coords in the header are informational only.
    let (cell_lat, cell_lon) = cell_coords_from_path(output_path);
    let bytes = write_cell(cell_lat, cell_lon, &[(TAG_ROAD, &cell_features)]);
    fs::write(output_path, bytes).map_err(|error| error.to_string())?;

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
    if cells.is_empty() {
        return None;
    }

    let mut any_file = false;
    let mut missing_cell = false;
    let mut seen_way_ids = HashSet::new();
    let mut roads = Vec::new();

    for (cell_lat, cell_lon) in cells {
        let path = vector_cell_path(&cache_dir, cell_lat, cell_lon);

        // Try binary format first.
        if path.exists() {
            any_file = true;
            if let Ok(data) = fs::read(&path) {
                if let Some(features) = read_single_chunk(&data, TAG_ROAD) {
                    for f in features {
                        let road_class = decode_road_class(f.class).to_owned();
                        if !road_class_matches(&road_class, layer_kind)
                            || !seen_way_ids.insert(f.way_id)
                        {
                            continue;
                        }
                        if f.points.len() < 2 {
                            continue;
                        }
                        let points: Vec<GeoPoint> = f
                            .points
                            .into_iter()
                            .map(|p| GeoPoint {
                                lat: p.lat,
                                lon: p.lon,
                            })
                            .collect();
                        if !bounds_intersect(polyline_bounds(&points), bounds) {
                            continue;
                        }
                        roads.push(RoadPolyline {
                            way_id: f.way_id,
                            road_class,
                            name: f.name,
                            points,
                        });
                    }
                    continue; // binary cell handled
                }
            }
        }

        // Legacy GeoJSON fallback.
        let geojson_path =
            cache_dir.join(format!("road_cell_{cell_lat:+04}_{cell_lon:+05}.geojson"));
        if !geojson_path.exists() {
            missing_cell = true;
            continue;
        }
        any_file = true;
        let Ok(body) = fs::read_to_string(&geojson_path) else {
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
            if !bounds_intersect(polyline_bounds(&points), bounds) {
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

    if !any_file || missing_cell {
        None
    } else {
        Some(roads)
    }
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

        // Load existing roads for merge (binary-first with GeoJSON fallback).
        let mut merged: HashMap<i64, RoadPolyline> =
            load_all_roads_from_vector_cell(&path, cell_lat, cell_lon, &cache_dir)
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

        let cell_features: Vec<CellFeature> = merged
            .into_values()
            .map(|road| CellFeature {
                way_id: road.way_id,
                class: encode_road_class(&road.road_class),
                is_polygon: false,
                name: road.name,
                points: road
                    .points
                    .into_iter()
                    .map(|p| CellPoint {
                        lon: p.lon,
                        lat: p.lat,
                    })
                    .collect(),
                elevations: None,
            })
            .collect();

        let bytes = write_cell(
            cell_lat as i16,
            cell_lon as i16,
            &[(TAG_ROAD, &cell_features)],
        );
        fs::write(&path, bytes).map_err(|e| e.to_string())?;
        written_cells += 1;
    }

    Ok(written_cells)
}

// ── Internal helpers ──────────────────────────────────────────────────────────

fn focus_cells_for_bounds(bounds: GeoBounds) -> Vec<(i32, i32)> {
    let min_lat_c = bounds.min_lat.floor() as i32;
    let max_lat_c = bounds.max_lat.floor() as i32;
    let min_lon_c = bounds.min_lon.floor() as i32;
    let max_lon_c = bounds.max_lon.floor() as i32;
    (min_lat_c..=max_lat_c)
        .flat_map(|lat| (min_lon_c..=max_lon_c).map(move |lon| (lat, lon)))
        .collect()
}

/// Load roads from a binary cell file, falling back to the legacy GeoJSON.
fn load_all_roads_from_vector_cell(
    binary_path: &Path,
    cell_lat: i32,
    cell_lon: i32,
    cache_dir: &Path,
) -> Option<Vec<RoadPolyline>> {
    if binary_path.exists() {
        if let Ok(data) = fs::read(binary_path) {
            if let Some(features) = read_single_chunk(&data, TAG_ROAD) {
                return Some(
                    features
                        .into_iter()
                        .filter(|f| f.points.len() >= 2)
                        .map(|f| RoadPolyline {
                            way_id: f.way_id,
                            road_class: decode_road_class(f.class).to_owned(),
                            name: f.name,
                            points: f
                                .points
                                .into_iter()
                                .map(|p| GeoPoint {
                                    lat: p.lat,
                                    lon: p.lon,
                                })
                                .collect(),
                        })
                        .collect(),
                );
            }
        }
    }

    // Legacy GeoJSON fallback.
    let geojson_path = cache_dir.join(format!("road_cell_{cell_lat:+04}_{cell_lon:+05}.geojson"));
    load_roads_from_geojson(&geojson_path)
}

fn load_roads_from_geojson(path: &Path) -> Option<Vec<RoadPolyline>> {
    let body = fs::read_to_string(path).ok()?;
    let payload = serde_json::from_str::<Value>(&body).ok()?;
    let features = payload.get("features").and_then(Value::as_array)?;
    let mut roads = Vec::new();

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
        let geometry = feature.get("geometry")?;
        let points = match geometry.get("type").and_then(Value::as_str) {
            Some("LineString") => parse_geojson_linestring(geometry),
            Some("MultiLineString") => {
                parse_geojson_multilinestring(geometry).and_then(|mut lines| lines.drain(..).next())
            }
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

/// Extract cell lat/lon from a `.1kc` filename as a best-effort fallback.
/// Returns (0, 0) on parse failure — the header coords are informational only.
fn cell_coords_from_path(path: &Path) -> (i16, i16) {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    // Expected stem: "road_cell_{:+04}_{:+05}"
    let parts: Vec<&str> = stem.split('_').collect();
    if parts.len() >= 4 {
        let lat = parts[parts.len() - 2].parse::<i16>().unwrap_or(0);
        let lon = parts[parts.len() - 1].parse::<i16>().unwrap_or(0);
        return (lat, lon);
    }
    (0, 0)
}
