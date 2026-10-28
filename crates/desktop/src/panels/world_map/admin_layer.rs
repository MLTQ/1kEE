//! Administrative cache discovery and immutable background snapshots.
use super::admin_geometry::{self, Fill};
use crate::model::GeoPoint;
use cell_format::{CellFeature, CellPoint, TAG_ADMN, admin_filename, read::read_single_chunk};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

pub(super) struct Region {
    pub relation_id: i64,
    pub level: u8,
    pub rings: Vec<Vec<GeoPoint>>,
    pub fill: Option<Fill>,
    pub bounds: [f32; 4], // min_lon, min_lat, max_lon, max_lat (unwrapped)
}
#[derive(Default)]
pub(super) struct AdminData {
    pub regions: Vec<Region>,
    pub missing: Vec<u8>,
    pub warnings: Vec<String>,
}
type AdminCache = super::layer_snapshot::Snapshot<(PathBuf, Vec<u8>), AdminData>;
static ADMIN_CACHE: OnceLock<Mutex<AdminCache>> = OnceLock::new();

pub(super) fn get(root: &Path, levels: &[u8]) -> Option<Arc<AdminData>> {
    if levels.is_empty() {
        return None;
    }
    let mut levels = levels.to_vec();
    levels.sort_unstable();
    levels.dedup();
    ADMIN_CACHE
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .get(
            (root.to_owned(), levels),
            "admin-boundaries",
            |a, b| a == b,
            |(root, levels)| {
                Some(load_from_dirs(
                    &super::cell_loader::osm_cache_dir_candidates(&root),
                    &levels,
                ))
            },
        )
}
pub(super) fn reload() {
    if let Some(cache) = ADMIN_CACHE.get() {
        cache.lock().unwrap().clear();
    }
}

fn load_from_dirs(dirs: &[PathBuf], levels: &[u8]) -> AdminData {
    let mut data = AdminData::default();
    for &level in levels {
        let mut loaded = None;
        for root in dirs {
            let binary = root.join("admin_cells").join(admin_filename(level));
            match fs::read(&binary) {
                Ok(bytes) => match read_single_chunk(&bytes, TAG_ADMN) {
                    Some(features) => {
                        loaded = Some(features);
                        break;
                    }
                    None => data
                        .warnings
                        .push(format!("Invalid boundary cache: {}", binary.display())),
                },
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => data.warnings.push(format!("{}: {e}", binary.display())),
            }
            let legacy = root
                .join("admin_cells")
                .join(format!("admin_level_{level}.geojson"));
            match fs::read(&legacy) {
                Ok(bytes) => match parse_geojson(&bytes, level) {
                    Ok(features) => {
                        loaded = Some(features);
                        break;
                    }
                    Err(error) => data.warnings.push(format!("{}: {error}", legacy.display())),
                },
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => data.warnings.push(format!("{}: {e}", legacy.display())),
            }
        }
        let Some(features) = loaded else {
            data.missing.push(level);
            continue;
        };
        let mut groups: BTreeMap<i64, (Vec<Vec<GeoPoint>>, bool)> = BTreeMap::new();
        for f in features {
            let entry = groups.entry(f.way_id).or_insert_with(|| (Vec::new(), true));
            if f.class != level
                || f.points.len() < 2
                || f.points.iter().any(|p| {
                    !p.lat.is_finite()
                        || !p.lon.is_finite()
                        || p.lat.abs() > 90.0
                        || p.lon.abs() > 180.0
                })
            {
                entry.1 = false;
                data.warnings.push(format!(
                    "Invalid geometry in level {level}, relation {}",
                    f.way_id
                ));
                continue;
            }
            let ring: Vec<_> = f
                .points
                .into_iter()
                .map(|p| GeoPoint {
                    lat: p.lat,
                    lon: p.lon,
                })
                .collect();
            entry.1 &= f.is_polygon && admin_geometry::closed(&ring);
            entry.0.push(ring);
        }
        for (relation_id, (mut rings, complete)) in groups {
            if rings.is_empty() {
                continue;
            }
            admin_geometry::unwrap_rings(&mut rings);
            let fill = if complete {
                match admin_geometry::tessellate(&rings) {
                    Ok(fill) => Some(fill),
                    Err(error) => {
                        data.warnings
                            .push(format!("Relation {relation_id}: {error}"));
                        None
                    }
                }
            } else {
                None
            };
            let mut bounds = [
                f32::INFINITY,
                f32::INFINITY,
                f32::NEG_INFINITY,
                f32::NEG_INFINITY,
            ];
            for p in rings.iter().flatten() {
                bounds[0] = bounds[0].min(p.lon);
                bounds[1] = bounds[1].min(p.lat);
                bounds[2] = bounds[2].max(p.lon);
                bounds[3] = bounds[3].max(p.lat);
            }
            data.regions.push(Region {
                relation_id,
                level,
                rings,
                fill,
                bounds,
            });
        }
    }
    data
}

fn parse_geojson(bytes: &[u8], level: u8) -> Result<Vec<CellFeature>, String> {
    let payload: Value = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    let features = payload
        .get("features")
        .and_then(Value::as_array)
        .ok_or("Expected a FeatureCollection")?;
    let mut result = Vec::new();
    for (index, feature) in features.iter().enumerate() {
        let props = &feature["properties"];
        let id = props["relation_id"]
            .as_i64()
            .or_else(|| feature["id"].as_i64())
            .unwrap_or(-(index as i64) - 1);
        let geometry = &feature["geometry"];
        let coords = &geometry["coordinates"];
        let mut rings: Vec<(&Value, bool)> = Vec::new();
        match geometry["type"].as_str().unwrap_or("") {
            "LineString" => rings.push((coords, false)),
            "MultiLineString" => rings.extend(
                coords
                    .as_array()
                    .ok_or("Invalid lines")?
                    .iter()
                    .map(|r| (r, false)),
            ),
            "Polygon" => rings.extend(
                coords
                    .as_array()
                    .ok_or("Invalid polygon")?
                    .iter()
                    .map(|r| (r, true)),
            ),
            "MultiPolygon" => {
                for polygon in coords.as_array().ok_or("Invalid polygons")? {
                    rings.extend(
                        polygon
                            .as_array()
                            .ok_or("Invalid polygon")?
                            .iter()
                            .map(|r| (r, true)),
                    );
                }
            }
            _ => continue,
        }
        for (ring, is_polygon) in rings {
            let points = ring
                .as_array()
                .ok_or("Invalid ring")?
                .iter()
                .map(|p| {
                    Ok(CellPoint {
                        lon: p[0].as_f64().ok_or("Invalid longitude")? as f32,
                        lat: p[1].as_f64().ok_or("Invalid latitude")? as f32,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?;
            result.push(CellFeature {
                way_id: id,
                class: level,
                is_polygon,
                name: props["name"].as_str().map(str::to_owned),
                points,
                elevations: None,
            });
        }
    }
    Ok(result)
}

#[cfg(test)]
#[path = "admin_layer_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "admin_preview.rs"]
mod preview;
