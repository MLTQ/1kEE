use crate::srtm::SrtmSampler;
use crate::util::{GeoPoint, RoadPolyline, WayFeature, bounds_intersect, focus_cell_bounds};
use cell_format::{
    CellFeature, CellPoint, TAG_ADMN, TAG_BLDG, TAG_ROAD, TAG_TREE, TAG_WATR,
    cell_filename, admin_filename,
    encode_road_class, encode_watr_class,
    read::read_single_chunk,
    write::write_cell,
};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

pub fn ensure_cache_dir(path: &Path) -> Result<(), String> {
    fs::create_dir_all(path).map_err(|error| error.to_string())
}

pub fn vector_cell_path(cache_dir: &Path, cell_lat: i32, cell_lon: i32) -> PathBuf {
    cache_dir
        .join("road_cells")
        .join(cell_filename("road", cell_lat, cell_lon))
}

pub fn feature_cell_path(cache_dir: &Path, prefix: &str, cell_lat: i32, cell_lon: i32) -> PathBuf {
    cache_dir
        .join(format!("{prefix}_cells"))
        .join(cell_filename(prefix, cell_lat, cell_lon))
}

// ── Road cells ───────────────────────────────────────────────────────────────

pub fn merge_write_cells(
    cache_dir: &Path,
    roads_by_cell: &HashMap<(i32, i32), Vec<RoadPolyline>>,
    mut srtm: Option<&mut SrtmSampler>,
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
        let mut features: Vec<CellFeature> = Vec::new();
        for road in merged.into_values() {
            if !bounds_intersect(crate::util::polyline_bounds(&road.points), cell_bounds) {
                continue;
            }
            let elevations = if let Some(s) = srtm.as_mut() {
                Some(road.points.iter().map(|p| s.sample(p.lat, p.lon)).collect::<Vec<_>>())
            } else {
                None
            };
            features.push(road_to_cell_feature(road, elevations));
        }

        let bytes = write_cell(cell_lat as i16, cell_lon as i16, &[(TAG_ROAD, &features)]);
        fs::write(&path, bytes).map_err(|e| e.to_string())?;
        written_cells += 1;
    }

    Ok(written_cells)
}

// ── Feature cells (buildings, waterways, trees) ──────────────────────────────

pub fn merge_write_feature_cells(
    cache_dir: &Path,
    prefix: &str,
    features_by_cell: &HashMap<(i32, i32), Vec<WayFeature>>,
    mut srtm: Option<&mut SrtmSampler>,
) -> Result<usize, String> {
    let cells_dir = cache_dir.join(format!("{prefix}_cells"));
    fs::create_dir_all(&cells_dir).map_err(|e| e.to_string())?;
    let mut written_cells = 0usize;

    let tag = prefix_to_tag(prefix);

    for (&(cell_lat, cell_lon), features) in features_by_cell {
        let path = feature_cell_path(cache_dir, prefix, cell_lat, cell_lon);
        let mut merged: HashMap<i64, WayFeature> = load_all_features_from_cell(&path, tag)
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
        let mut cell_features: Vec<CellFeature> = Vec::new();
        for f in merged.into_values() {
            if !bounds_intersect(crate::util::polyline_bounds(&f.points), cell_bounds) {
                continue;
            }
            let elevations = if let Some(s) = srtm.as_mut() {
                Some(f.points.iter().map(|p| s.sample(p.lat, p.lon)).collect::<Vec<_>>())
            } else {
                None
            };
            cell_features.push(way_feature_to_cell_feature_with_elev(&f, prefix, elevations));
        }

        let bytes = write_cell(cell_lat as i16, cell_lon as i16, &[(tag, &cell_features)]);
        fs::write(&path, bytes).map_err(|e| e.to_string())?;
        written_cells += 1;
    }

    Ok(written_cells)
}

// ── Admin boundary files ──────────────────────────────────────────────────────

/// Write (or overwrite) a per-admin-level binary file.
///
/// `features`: `(relation_id, name, rings)` — each ring becomes its own
/// `CellFeature` (matching prior GeoJSON behaviour where each ring was a
/// separate LineString Feature).
///
/// Writes to `{cache_dir}/admin_cells/admin_level_{admin_level}.1kc`.
/// Returns the total number of ring features written.
pub fn write_admin_level_file(
    cache_dir: &Path,
    admin_level: u8,
    features: &[(i64, Option<String>, Vec<Vec<GeoPoint>>)],
) -> Result<usize, String> {
    let admin_dir = cache_dir.join("admin_cells");
    fs::create_dir_all(&admin_dir).map_err(|e| e.to_string())?;
    let path = admin_dir.join(admin_filename(admin_level));

    let mut cell_features: Vec<CellFeature> = Vec::new();
    for (relation_id, name, rings) in features {
        for ring in rings {
            if ring.len() < 2 {
                continue;
            }
            cell_features.push(CellFeature {
                way_id: *relation_id,
                class: admin_level,
                is_polygon: false,
                name: name.clone(),
                points: ring
                    .iter()
                    .map(|pt| CellPoint { lon: pt.lon, lat: pt.lat })
                    .collect(),
                elevations: None,
            });
        }
    }

    let count = cell_features.len();
    // Admin files are global, not per-cell, so cell coordinates are (0, 0).
    let bytes = write_cell(0, 0, &[(TAG_ADMN, &cell_features)]);
    fs::write(&path, bytes).map_err(|e| e.to_string())?;
    Ok(count)
}

// ── Read-back helpers (for merge; fall back to legacy GeoJSON if needed) ──────

/// Load roads from a binary cell file, falling back to legacy GeoJSON.
pub fn load_all_roads_from_vector_cell(path: &Path) -> Option<Vec<RoadPolyline>> {
    if path.exists() {
        if let Some(roads) = load_roads_from_binary(path) {
            return Some(roads);
        }
    }
    // Legacy GeoJSON fallback — lets the builder migrate existing caches.
    let geojson_path = path.with_extension("geojson");
    load_roads_from_geojson(&geojson_path)
}

fn load_roads_from_binary(path: &Path) -> Option<Vec<RoadPolyline>> {
    let data = fs::read(path).ok()?;
    let features = read_single_chunk(&data, TAG_ROAD)?;
    Some(
        features
            .into_iter()
            .filter(|f| f.points.len() >= 2)
            .map(|f| RoadPolyline {
                way_id: f.way_id,
                road_class: cell_format::decode_road_class(f.class).to_owned(),
                name: f.name,
                points: f.points.into_iter().map(|p| GeoPoint { lat: p.lat, lon: p.lon }).collect(),
            })
            .collect(),
    )
}

/// Load features from a binary cell file, falling back to legacy GeoJSON.
pub fn load_all_features_from_cell(path: &Path, tag: [u8; 4]) -> Option<Vec<WayFeature>> {
    if path.exists() {
        if let Some(features) = load_features_from_binary(path, tag) {
            return Some(features);
        }
    }
    let geojson_path = path.with_extension("geojson");
    load_features_from_geojson(&geojson_path)
}

fn load_features_from_binary(path: &Path, tag: [u8; 4]) -> Option<Vec<WayFeature>> {
    let data = fs::read(path).ok()?;
    let features = read_single_chunk(&data, tag)?;
    let prefix = tag_to_prefix(tag);
    Some(
        features
            .into_iter()
            .filter(|f| f.points.len() >= 2)
            .map(|f| {
                let feature_class = if tag == TAG_WATR {
                    cell_format::decode_watr_class(f.class).to_owned()
                } else if tag == TAG_BLDG {
                    "building".to_owned()
                } else if tag == TAG_TREE {
                    "forest".to_owned()
                } else {
                    prefix.to_owned()
                };
                WayFeature {
                    way_id: f.way_id,
                    feature_class,
                    name: f.name,
                    points: f.points.into_iter().map(|p| GeoPoint { lat: p.lat, lon: p.lon }).collect(),
                    is_polygon: f.is_polygon,
                }
            })
            .collect(),
    )
}

// ── Conversion helpers ────────────────────────────────────────────────────────

fn road_to_cell_feature(road: RoadPolyline, elevations: Option<Vec<f32>>) -> CellFeature {
    CellFeature {
        way_id: road.way_id,
        class: encode_road_class(&road.road_class),
        is_polygon: false,
        name: road.name,
        points: road.points.into_iter().map(|p| CellPoint { lon: p.lon, lat: p.lat }).collect(),
        elevations,
    }
}

fn way_feature_to_cell_feature_with_elev(
    f: &WayFeature,
    prefix: &str,
    elevations: Option<Vec<f32>>,
) -> CellFeature {
    let class = match prefix {
        "waterway" => encode_watr_class(&f.feature_class),
        _ => 0, // building=0, forest=0, etc.
    };
    CellFeature {
        way_id: f.way_id,
        class,
        is_polygon: f.is_polygon,
        name: f.name.clone(),
        points: f.points.iter().map(|p| CellPoint { lon: p.lon, lat: p.lat }).collect(),
        elevations,
    }
}

fn way_feature_to_cell_feature(f: &WayFeature, prefix: &str) -> CellFeature {
    way_feature_to_cell_feature_with_elev(f, prefix, None)
}

fn prefix_to_tag(prefix: &str) -> [u8; 4] {
    match prefix {
        "waterway" => TAG_WATR,
        "building" => TAG_BLDG,
        "tree" => TAG_TREE,
        _ => TAG_BLDG,
    }
}

fn tag_to_prefix(tag: [u8; 4]) -> &'static str {
    match &tag {
        b"WATR" => "waterway",
        b"BLDG" => "building",
        b"TREE" => "tree",
        _ => "unknown",
    }
}

// ── Legacy GeoJSON read-back (migration only, not called for fresh caches) ────

fn load_roads_from_geojson(path: &Path) -> Option<Vec<RoadPolyline>> {
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
        let coordinates = feature
            .get("geometry")
            .and_then(|g| g.get("coordinates"))
            .and_then(Value::as_array)?;
        let mut points = Vec::with_capacity(coordinates.len());
        for coord in coordinates {
            let pair = coord.as_array()?;
            let lon = pair.first()?.as_f64()? as f32;
            let lat = pair.get(1)?.as_f64()? as f32;
            points.push(GeoPoint { lat, lon });
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
                .filter(|n| !n.is_empty()),
            points,
        });
    }

    Some(roads)
}

fn load_features_from_geojson(path: &Path) -> Option<Vec<WayFeature>> {
    let body = fs::read_to_string(path).ok()?;
    let payload = serde_json::from_str::<Value>(&body).ok()?;
    let features = payload.get("features").and_then(Value::as_array)?;
    let mut result = Vec::new();

    for feature in features {
        let props = feature.get("properties").unwrap_or(&Value::Null);
        let way_id = props.get("way_id").and_then(Value::as_i64).unwrap_or_default();
        let feature_class = props
            .get("class")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let geometry = feature.get("geometry")?;
        let geom_type = geometry.get("type").and_then(Value::as_str).unwrap_or("");
        let is_polygon = geom_type == "Polygon";
        let raw_coords = geometry.get("coordinates").and_then(Value::as_array)?;

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
            points.push(GeoPoint { lat, lon });
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
                .filter(|n| !n.is_empty()),
            points,
            is_polygon,
        });
    }

    Some(result)
}
