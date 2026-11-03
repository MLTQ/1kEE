//! Administrative cache discovery and immutable background snapshots.
#[path = "admin_view.rs"]
pub(super) mod view;
use super::admin_geometry::{self, Fill};
use crate::model::GeoPoint;
#[cfg(test)]
use cell_format::TAG_ADMN;
use cell_format::{CellFeature, CellPoint, admin_filename};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
};
use tile_archive::admin_lod::{self, Archive, Window};
use view::Request;

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
    pub shadeable: usize,
    pub missing: Vec<u8>,
    pub warnings: Vec<String>,
}
type Key = (
    Vec<PathBuf>,
    Vec<u8>,
    super::admin_source::Fingerprint,
    Request,
);
type AdminCache = super::layer_snapshot::Snapshot<Key, AdminData>;
static ADMIN_CACHE: OnceLock<Mutex<AdminCache>> = OnceLock::new();

/// The drawer observes the renderer's snapshot; it cannot request a competing view.
pub(super) fn displayed(model: &crate::model::AppModel) -> Option<Arc<AdminData>> {
    let cache = ADMIN_CACHE.get()?.lock().unwrap();
    let (key, data) = cache.current()?;
    (key.0 == super::admin_source::directories(model) && key.1 == model.admin_options.levels())
        .then(|| Arc::clone(data))
}

pub(super) fn get_for(model: &crate::model::AppModel, request: Request) -> Option<Arc<AdminData>> {
    let dirs = super::admin_source::directories(model);
    let levels = model.admin_options.levels();
    if dirs.is_empty() || levels.is_empty() {
        return None;
    }
    let status = super::admin_source::poll(&dirs)?;
    let files = status
        .files
        .iter()
        .filter(|(path, _, _)| {
            path.file_stem()
                .and_then(|s| s.to_str())
                .and_then(|s| s.strip_prefix("admin_level_"))
                .and_then(|s| s.parse::<u8>().ok())
                .is_some_and(|level| levels.contains(&level))
        })
        .cloned()
        .collect();
    ADMIN_CACHE
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .get(
            (dirs, levels, files, request),
            "admin-boundaries",
            |a, b| a.0 == b.0 && a.1 == b.1 && a.2 == b.2,
            |(dirs, levels, _, request)| Some(load_detail(&dirs, &levels, request)),
        )
}
pub(super) fn reload() {
    if let Some(cache) = ADMIN_CACHE.get() {
        cache.lock().unwrap().clear();
    }
}

#[cfg(test)]
fn load_from_dirs(dirs: &[PathBuf], levels: &[u8]) -> AdminData {
    load_detail(
        dirs,
        levels,
        Request {
            tier: admin_lod::FULL,
            window: Window::WORLD,
            shade: true,
        },
    )
}

type Groups = BTreeMap<i64, (Vec<Vec<GeoPoint>>, bool)>;
fn add(groups: &mut Groups, id: i64, complete: bool, points: Vec<CellPoint>) {
    let entry = groups.entry(id).or_insert_with(|| (Vec::new(), true));
    entry.1 &= complete;
    entry.0.push(
        points
            .into_iter()
            .map(|p| GeoPoint {
                lat: p.lat,
                lon: p.lon,
            })
            .collect(),
    );
}
fn legacy_groups(
    features: Vec<CellFeature>,
    request: Request,
    warnings: &mut Vec<String>,
) -> Groups {
    let mut groups = Groups::new();
    for f in features {
        let group = groups.entry(f.way_id).or_insert_with(|| (Vec::new(), true));
        if f.points.len() < 2
            || f.points.iter().any(|p| {
                !p.lat.is_finite() || !p.lon.is_finite() || p.lat.abs() > 90. || p.lon.abs() > 180.
            })
        {
            group.1 = false;
            warnings.push(format!(
                "Invalid boundary geometry in relation {}",
                f.way_id
            ));
            continue;
        }
        let complete = f.is_polygon && f.points.len() >= 4 && f.points.first() == f.points.last();
        group.1 &= complete;
        let mut pts = f.points;
        let anchor = pts[0].lon;
        admin_lod::unwrap(&mut pts, anchor);
        if !request.window.intersects(admin_lod::bounds(&pts)) {
            continue;
        }
        if request.tier != admin_lod::FULL {
            pts = admin_lod::simplify(&pts, &admin_lod::importance(&pts), request.tier);
        }
        add(&mut groups, f.way_id, complete, pts);
    }
    groups
}
fn load_level(
    dirs: &[PathBuf],
    level: u8,
    request: Request,
    warnings: &mut Vec<String>,
) -> Option<Groups> {
    for root in dirs {
        let dir = root.join("admin_cells");
        let binary = dir.join(admin_filename(level));
        let mut groups = Groups::new();
        match Archive::open(&binary, level).and_then(|mut archive| {
            if archive.invalid_rings > 0 {
                warnings.push(format!(
                    "{}: skipped {} invalid boundary rings",
                    binary.display(),
                    archive.invalid_rings
                ));
            }
            archive.query(request.tier, request.window, request.shade, |p| {
                add(&mut groups, p.relation, p.complete, p.points)
            })
        }) {
            Ok(()) => return Some(groups),
            Err(e)
                if e.downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) => {}
            Err(e) => warnings.push(format!("{}: {e}", binary.display())),
        }
        let legacy = dir.join(format!("admin_level_{level}.geojson"));
        match fs::read(&legacy) {
            Ok(bytes) => match parse_geojson(&bytes, level) {
                Ok(features) => return Some(legacy_groups(features, request, warnings)),
                Err(e) => warnings.push(format!("{}: {e}", legacy.display())),
            },
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                warnings.push(format!("{}: {e}", legacy.display()))
            }
            Err(_) => {}
        }
    }
    None
}
fn load_detail(dirs: &[PathBuf], levels: &[u8], request: Request) -> AdminData {
    let mut data = AdminData::default();
    for &level in levels {
        let Some(groups) = load_level(dirs, level, request, &mut data.warnings) else {
            data.missing.push(level);
            continue;
        };
        for (relation_id, (mut rings, complete)) in groups {
            if rings.is_empty() {
                continue;
            }
            admin_geometry::align_rings(&mut rings);
            let bounds = geo_bounds(rings.iter().flatten());
            let fill = if request.shade && complete {
                match admin_geometry::tessellate(&rings) {
                    Ok(fill) => Some(visible_fill(fill, request.window)),
                    Err(e) => {
                        data.warnings.push(format!("Relation {relation_id}: {e}"));
                        None
                    }
                }
            } else {
                None
            };
            if request.window != Window::WORLD
                && (request.shade || rings.iter().any(|r| r.len() > 257))
            {
                // Fills need intact rings. Afterwards retain only border pieces near this view.
                let mut outline = Vec::new();
                for ring in rings {
                    for start in (0..ring.len() - 1).step_by(256) {
                        let part = &ring[start..ring.len().min(start + 257)];
                        if request.window.intersects(geo_bounds(part.iter())) {
                            outline.push(part.to_vec());
                        }
                    }
                }
                rings = outline;
            }
            if complete && (!request.shade || fill.is_some()) {
                data.shadeable += 1;
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
fn geo_bounds<'a>(points: impl Iterator<Item = &'a GeoPoint>) -> [f32; 4] {
    points.fold(
        [
            f32::INFINITY,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::NEG_INFINITY,
        ],
        |b, p| {
            [
                b[0].min(p.lon),
                b[1].min(p.lat),
                b[2].max(p.lon),
                b[3].max(p.lat),
            ]
        },
    )
}
fn visible_fill(fill: Fill, window: Window) -> Fill {
    if window == Window::WORLD {
        return fill;
    }
    let mut out = Fill::default();
    let mut compact = vec![u32::MAX; fill.vertices.len()];
    for tri in fill.indices.chunks_exact(3) {
        if !window.intersects(geo_bounds(tri.iter().map(|&i| &fill.vertices[i as usize]))) {
            continue;
        }
        for &i in tri {
            let idx = &mut compact[i as usize];
            if *idx == u32::MAX {
                *idx = out.vertices.len() as u32;
                out.vertices.push(fill.vertices[i as usize]);
            }
            out.indices.push(*idx);
        }
    }
    out
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
