use crate::model::GeoPoint;
use crate::osm_ingest::GeoBounds;
use cell_format::{
    TAG_BLDG, TAG_TREE, TAG_WATR, cell_filename, decode_class, read::read_single_chunk,
};
use serde_json::Value;
use std::fs;
use std::path::Path;

/// A generic polyline / polygon feature loaded from a per-cell binary file.
pub struct LoadedPolyline {
    pub way_id: i64,
    pub class: String,
    pub name: Option<String>,
    pub points: Vec<GeoPoint>,
    pub is_polygon: bool,
}

/// Load all features for `bounds` from per-cell binary files stored under
/// `root/{prefix}_cells/{prefix}_cell_{lat}_{lon}.1kc`, falling back to
/// legacy `.geojson` files for cells that have not yet been rebuilt.
pub fn load_features_from_cells(
    root: &Path,
    prefix: &str,
    bounds: GeoBounds,
) -> Vec<LoadedPolyline> {
    let cell_dir = root.join(format!("{prefix}_cells"));
    if !cell_dir.exists() {
        return Vec::new();
    }

    let tag = prefix_to_tag(prefix);

    let min_lat_c = bounds.min_lat.floor() as i32;
    let max_lat_c = bounds.max_lat.floor() as i32;
    let min_lon_c = bounds.min_lon.floor() as i32;
    let max_lon_c = bounds.max_lon.floor() as i32;

    let mut results = Vec::new();

    for lat in min_lat_c..=max_lat_c {
        for lon in min_lon_c..=max_lon_c {
            // Try binary format first, then fall back to GeoJSON.
            let binary_path = cell_dir.join(cell_filename(prefix, lat, lon));
            if binary_path.exists() {
                if let Ok(data) = fs::read(&binary_path) {
                    if let Some(features) = read_single_chunk(&data, tag) {
                        for f in features {
                            if f.points.len() < 2 {
                                continue;
                            }
                            results.push(LoadedPolyline {
                                way_id: f.way_id,
                                class: decode_class(&tag, f.class).to_owned(),
                                name: f.name,
                                points: f
                                    .points
                                    .into_iter()
                                    .map(|p| GeoPoint {
                                        lat: p.lat,
                                        lon: p.lon,
                                    })
                                    .collect(),
                                is_polygon: f.is_polygon,
                            });
                        }
                        continue; // binary cell loaded — skip GeoJSON fallback
                    }
                }
            }

            // Legacy GeoJSON fallback.
            let geojson_path = cell_dir.join(format!("{prefix}_cell_{lat:+04}_{lon:+05}.geojson"));
            if !geojson_path.exists() {
                continue;
            }
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

fn prefix_to_tag(prefix: &str) -> [u8; 4] {
    match prefix {
        "waterway" => TAG_WATR,
        "building" => TAG_BLDG,
        "tree" => TAG_TREE,
        _ => TAG_BLDG,
    }
}

// ── GeoJSON geometry parsers (legacy fallback only) ───────────────────────────

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
