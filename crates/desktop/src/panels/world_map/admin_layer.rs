// Loads administrative boundary files from the cache.
// One file per admin level (2=country, 4=state, 6=county, 8=municipality).
// Files are loaded lazily on first draw call, cached for the session.
// Reads binary .1kc format, falling back to legacy .geojson.

use crate::model::GeoPoint;
use cell_format::{TAG_ADMN, admin_filename, read::read_single_chunk};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

// ── Public types ───────────────────────────────────────────────────────────────

pub struct LoadedAdminBoundary {
    pub relation_id: i64,
    pub admin_level: u8,
    pub name: Option<String>,
    pub points: Vec<GeoPoint>,
}

// ── Session cache ──────────────────────────────────────────────────────────────

struct AdminCache {
    loaded_root: Option<PathBuf>,
    boundaries: Vec<LoadedAdminBoundary>,
}

static ADMIN_CACHE: OnceLock<Mutex<AdminCache>> = OnceLock::new();

/// Return all admin boundaries for `levels`, loading from disk only when the
/// cache root changes (or on first call).  Clones out of the mutex so callers
/// hold no lock during rendering.
pub fn get_or_load_admin_boundaries(cache_root: &Path, levels: &[u8]) -> Vec<LoadedAdminBoundary> {
    let cache = ADMIN_CACHE.get_or_init(|| {
        Mutex::new(AdminCache {
            loaded_root: None,
            boundaries: Vec::new(),
        })
    });
    let mut guard = cache.lock().unwrap();
    if guard.loaded_root.as_deref() != Some(cache_root) {
        guard.boundaries = load_admin_boundaries(cache_root, levels);
        guard.loaded_root = Some(cache_root.to_owned());
    }
    // Clone each boundary individually (Vec<GeoPoint> is cheaply clonable at
    // the scale of whole-world admin boundaries which are loaded once).
    guard
        .boundaries
        .iter()
        .map(|b| LoadedAdminBoundary {
            relation_id: b.relation_id,
            admin_level: b.admin_level,
            name: b.name.clone(),
            points: b.points.clone(),
        })
        .collect()
}

// ── Loading ────────────────────────────────────────────────────────────────────

/// Read admin boundary files for each requested level, trying binary `.1kc`
/// first then falling back to legacy `.geojson`.
pub fn load_admin_boundaries(cache_root: &Path, levels: &[u8]) -> Vec<LoadedAdminBoundary> {
    let admin_dir = cache_root.join("admin_cells");
    if !admin_dir.exists() {
        return Vec::new();
    }

    let mut results = Vec::new();

    for &level in levels {
        // Binary path.
        let binary_path = admin_dir.join(admin_filename(level));
        if binary_path.exists() {
            if let Ok(data) = fs::read(&binary_path) {
                if let Some(features) = read_single_chunk(&data, TAG_ADMN) {
                    for f in features {
                        if f.points.len() < 2 {
                            continue;
                        }
                        results.push(LoadedAdminBoundary {
                            relation_id: f.way_id,
                            admin_level: f.class,
                            name: f.name,
                            points: f
                                .points
                                .into_iter()
                                .map(|p| GeoPoint {
                                    lat: p.lat,
                                    lon: p.lon,
                                })
                                .collect(),
                        });
                    }
                    continue; // binary level loaded
                }
            }
        }

        // Legacy GeoJSON fallback.
        let geojson_path = admin_dir.join(format!("admin_level_{level}.geojson"));
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

            let relation_id = props
                .get("relation_id")
                .and_then(Value::as_i64)
                .unwrap_or_default();
            let name = props
                .get("name")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
                .filter(|n| !n.is_empty());
            let admin_level_prop = props
                .get("admin_level")
                .and_then(Value::as_u64)
                .unwrap_or(level as u64) as u8;

            let Some(geometry) = feature.get("geometry") else {
                continue;
            };
            let geom_type = geometry.get("type").and_then(Value::as_str).unwrap_or("");
            if geom_type != "LineString" {
                continue;
            }

            let Some(points) = parse_linestring(geometry) else {
                continue;
            };
            if points.len() < 2 {
                continue;
            }

            results.push(LoadedAdminBoundary {
                relation_id,
                admin_level: admin_level_prop,
                name,
                points,
            });
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
