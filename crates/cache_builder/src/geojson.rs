use crate::util::{GeoBounds, RoadPolyline, WayFeature, bounds_intersect, focus_cell_bounds};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

pub fn ensure_cache_dir(path: &Path) -> Result<(), String> {
    fs::create_dir_all(path).map_err(|error| error.to_string())
}

pub fn vector_cell_path(cache_dir: &Path, cell_lat: i32, cell_lon: i32) -> PathBuf {
    cache_dir
        .join("road_cells")
        .join(format!(
            "road_cell_{:+04}_{:+05}.geojson",
            cell_lat, cell_lon
        ))
}

pub fn feature_cell_path(cache_dir: &Path, prefix: &str, cell_lat: i32, cell_lon: i32) -> PathBuf {
    cache_dir
        .join(format!("{prefix}_cells"))
        .join(format!("{prefix}_cell_{:+04}_{:+05}.geojson", cell_lat, cell_lon))
}

pub fn merge_write_cells(
    cache_dir: &Path,
    roads_by_cell: &HashMap<(i32, i32), Vec<RoadPolyline>>,
) -> Result<usize, String> {
    let road_cells_dir = cache_dir.join("road_cells");
    ensure_cache_dir(&road_cells_dir)?;
    let mut written_cells = 0usize;
    for (&(cell_lat, cell_lon), roads) in roads_by_cell {
        let path = vector_cell_path(cache_dir, cell_lat, cell_lon);
        let mut merged: HashMap<i64, RoadPolyline> = load_all_roads_from_vector_cell(&path)
            .unwrap_or_default()
            .into_iter()
            .map(|road| (road.way_id, road))
            .collect();
        let before = merged.len();
        for road in roads {
            merged.insert(road.way_id, road.clone());
        }
        if merged.len() == before && path.exists() {
            continue;
        }

        let cell_bounds = focus_cell_bounds(cell_lat, cell_lon);
        let body = feature_collection_for_cell(cell_bounds, merged.into_values().collect())?;
        fs::write(&path, body).map_err(|error| error.to_string())?;
        written_cells += 1;
    }
    Ok(written_cells)
}

pub fn merge_write_feature_cells(
    cache_dir: &Path,
    prefix: &str,
    features_by_cell: &HashMap<(i32, i32), Vec<WayFeature>>,
) -> Result<usize, String> {
    let cells_dir = cache_dir.join(format!("{prefix}_cells"));
    fs::create_dir_all(&cells_dir).map_err(|error| error.to_string())?;
    let mut written_cells = 0usize;
    for (&(cell_lat, cell_lon), features) in features_by_cell {
        let path = feature_cell_path(cache_dir, prefix, cell_lat, cell_lon);
        let mut merged: HashMap<i64, WayFeature> = load_all_features_from_cell(&path)
            .unwrap_or_default()
            .into_iter()
            .map(|f| (f.way_id, f))
            .collect();
        let before = merged.len();
        for feature in features {
            merged.insert(feature.way_id, feature.clone());
        }
        if merged.len() == before && path.exists() {
            continue;
        }

        let cell_bounds = focus_cell_bounds(cell_lat, cell_lon);
        let body = way_feature_collection_for_cell(cell_bounds, merged.into_values().collect())?;
        fs::write(&path, body).map_err(|error| error.to_string())?;
        written_cells += 1;
    }
    Ok(written_cells)
}

fn feature_collection_for_cell(
    bounds: GeoBounds,
    roads: Vec<RoadPolyline>,
) -> Result<String, String> {
    let mut features = Vec::new();
    for road in roads {
        if !bounds_intersect(crate::util::polyline_bounds(&road.points), bounds) {
            continue;
        }
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

    serde_json::to_string(&json!({
        "type": "FeatureCollection",
        "features": features,
    }))
    .map_err(|error| error.to_string())
}

fn way_feature_collection_for_cell(
    bounds: GeoBounds,
    features: Vec<WayFeature>,
) -> Result<String, String> {
    let mut out_features = Vec::new();
    for feature in features {
        if !bounds_intersect(crate::util::polyline_bounds(&feature.points), bounds) {
            continue;
        }
        let ring: Vec<Value> = feature
            .points
            .iter()
            .map(|point| json!([point.lon as f64, point.lat as f64]))
            .collect();
        let (geom_type, coordinates) = if feature.is_polygon {
            ("Polygon", json!([ring]))
        } else {
            ("LineString", json!(ring))
        };
        out_features.push(json!({
            "type": "Feature",
            "properties": {
                "way_id": feature.way_id,
                "class": feature.feature_class,
                "name": feature.name,
            },
            "geometry": {
                "type": geom_type,
                "coordinates": coordinates,
            }
        }));
    }

    serde_json::to_string(&json!({
        "type": "FeatureCollection",
        "features": out_features,
    }))
    .map_err(|error| error.to_string())
}

fn load_all_roads_from_vector_cell(path: &Path) -> Option<Vec<RoadPolyline>> {
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
        let coordinates = feature
            .get("geometry")
            .and_then(|geometry| geometry.get("coordinates"))
            .and_then(Value::as_array)?;
        let mut points = Vec::with_capacity(coordinates.len());
        for coord in coordinates {
            let pair = coord.as_array()?;
            let lon = pair.first()?.as_f64()? as f32;
            let lat = pair.get(1)?.as_f64()? as f32;
            points.push(crate::util::GeoPoint { lat, lon });
        }
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

fn load_all_features_from_cell(path: &Path) -> Option<Vec<WayFeature>> {
    let body = fs::read_to_string(path).ok()?;
    let payload = serde_json::from_str::<Value>(&body).ok()?;
    let features = payload.get("features").and_then(Value::as_array)?;
    let mut result = Vec::new();

    for feature in features {
        let props = feature.get("properties").unwrap_or(&Value::Null);
        let way_id = props
            .get("way_id")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        let feature_class = props
            .get("class")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let geometry = feature.get("geometry")?;
        let geom_type = geometry.get("type").and_then(Value::as_str).unwrap_or("");
        let is_polygon = geom_type == "Polygon";
        let raw_coords = geometry.get("coordinates").and_then(Value::as_array)?;

        // For Polygon, coordinates = [[ring]], for LineString coordinates = [coord, ...]
        let coord_array = if is_polygon {
            raw_coords.first()?.as_array()?
        } else {
            raw_coords
        };

        let mut points = Vec::with_capacity(coord_array.len());
        for coord in coord_array {
            let pair = coord.as_array()?;
            let lon = pair.first()?.as_f64()? as f32;
            let lat = pair.get(1)?.as_f64()? as f32;
            points.push(crate::util::GeoPoint { lat, lon });
        }
        if points.len() < 2 {
            continue;
        }
        result.push(WayFeature {
            way_id,
            feature_class,
            name: props
                .get("name")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
                .filter(|name| !name.is_empty()),
            points,
            is_polygon,
        });
    }

    Some(result)
}
