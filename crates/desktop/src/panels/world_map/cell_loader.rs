use crate::model::GeoPoint;
use crate::osm_ingest::GeoBounds;
use serde_json::Value;
use std::fs;
use std::path::Path;

/// A generic polyline / polygon feature loaded from a per-cell GeoJSON file.
pub struct LoadedPolyline {
    pub way_id: i64,
    pub class: String,
    pub name: Option<String>,
    pub points: Vec<GeoPoint>,
    pub is_polygon: bool,
}

/// Load all features for `bounds` from per-cell GeoJSON files stored under
/// `root/{prefix}_cells/{prefix}_cell_{lat}_{lon}.geojson`.
pub fn load_features_from_cells(
    root: &Path,
    prefix: &str,
    bounds: GeoBounds,
) -> Vec<LoadedPolyline> {
    let cell_dir = root.join(format!("{prefix}_cells"));
    if !cell_dir.exists() {
        return Vec::new();
    }

    let min_lat_c = bounds.min_lat.floor() as i32;
    let max_lat_c = bounds.max_lat.floor() as i32;
    let min_lon_c = bounds.min_lon.floor() as i32;
    let max_lon_c = bounds.max_lon.floor() as i32;

    let mut results = Vec::new();

    for lat in min_lat_c..=max_lat_c {
        for lon in min_lon_c..=max_lon_c {
            let path = cell_dir.join(format!("{prefix}_cell_{:+04}_{:+05}.geojson", lat, lon));
            if !path.exists() {
                continue;
            }
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
                let class = props
                    .get("class")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                let name = props
                    .get("name")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
                    .filter(|n| !n.is_empty());

                let Some(geometry) = feature.get("geometry") else {
                    continue;
                };
                let geom_type = geometry.get("type").and_then(Value::as_str).unwrap_or("");
                let is_polygon = geom_type == "Polygon";

                let points = match geom_type {
                    "LineString" => parse_linestring(geometry),
                    "Polygon" => parse_polygon_first_ring(geometry),
                    _ => None,
                };
                let Some(points) = points else {
                    continue;
                };
                if points.len() < 2 {
                    continue;
                }

                results.push(LoadedPolyline {
                    way_id,
                    class,
                    name,
                    points,
                    is_polygon,
                });
            }
        }
    }

    results
}

fn parse_linestring(geometry: &Value) -> Option<Vec<GeoPoint>> {
    let coords = geometry.get("coordinates").and_then(Value::as_array)?;
    let pts: Vec<GeoPoint> = coords
        .iter()
        .filter_map(|c| {
            let arr = c.as_array()?;
            let lon = arr.first()?.as_f64()? as f32;
            let lat = arr.get(1)?.as_f64()? as f32;
            Some(GeoPoint { lat, lon })
        })
        .collect();
    Some(pts)
}

fn parse_polygon_first_ring(geometry: &Value) -> Option<Vec<GeoPoint>> {
    let rings = geometry.get("coordinates").and_then(Value::as_array)?;
    let ring = rings.first().and_then(Value::as_array)?;
    let pts: Vec<GeoPoint> = ring
        .iter()
        .filter_map(|c| {
            let arr = c.as_array()?;
            let lon = arr.first()?.as_f64()? as f32;
            let lat = arr.get(1)?.as_f64()? as f32;
            Some(GeoPoint { lat, lon })
        })
        .collect();
    Some(pts)
}
