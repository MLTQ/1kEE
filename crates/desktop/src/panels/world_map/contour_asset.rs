use crate::model::GeoPoint;
use crate::terrain_assets;
use rusqlite::{Connection, params};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use super::srtm_focus_cache;

#[derive(Clone)]
pub struct ContourPath {
    pub elevation_m: f32,
    pub points: Vec<GeoPoint>,
}

#[derive(Clone, Hash, PartialEq, Eq)]
struct CacheKey {
    path: PathBuf,
    lat_bucket: i32,
    lon_bucket: i32,
    zoom_bucket: i32,
}

struct CachedContours {
    key: CacheKey,
    contours: Arc<Vec<ContourPath>>,
}

struct LocalRegionCache {
    scene_key: Option<SceneKey>,
    entries: HashMap<CacheKey, Arc<Vec<ContourPath>>>,
    retained_keys: Vec<CacheKey>,
}

#[derive(Clone, PartialEq, Eq)]
struct SceneKey {
    root: Option<PathBuf>,
    anchor_lat_bucket: i32,
    anchor_lon_bucket: i32,
    zoom_bucket: i32,
}

#[derive(Clone, Copy)]
struct GeoBounds {
    min_lat: f32,
    max_lat: f32,
    min_lon: f32,
    max_lon: f32,
}

pub fn load_srtm_for_focus(
    selected_root: Option<&Path>,
    focus: GeoPoint,
    zoom: f32,
) -> Option<Arc<Vec<ContourPath>>> {
    load_srtm_region_for_focus(selected_root, focus, zoom, 0)
}

pub fn load_srtm_region_for_focus(
    selected_root: Option<&Path>,
    focus: GeoPoint,
    zoom: f32,
    radius: i32,
) -> Option<Arc<Vec<ContourPath>>> {
    load_srtm_region_for_view(selected_root, focus, focus, zoom, radius)
}

pub fn load_srtm_region_for_view(
    selected_root: Option<&Path>,
    scene_anchor: GeoPoint,
    viewport_center: GeoPoint,
    zoom: f32,
    radius: i32,
) -> Option<Arc<Vec<ContourPath>>> {
    let assets =
        srtm_focus_cache::ensure_focus_contour_region(selected_root, viewport_center, zoom, radius);
    if assets.is_empty() {
        return None;
    }

    static CACHE: OnceLock<Mutex<LocalRegionCache>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| {
        Mutex::new(LocalRegionCache {
            scene_key: None,
            entries: HashMap::new(),
            retained_keys: Vec::new(),
        })
    });
    let mut guard = cache.lock().ok()?;
    let feature_budget = srtm_focus_cache::feature_budget_for_zoom(zoom);
    let per_asset_budget = (feature_budget / assets.len().max(1)).max(120);
    let scene_key = SceneKey {
        root: selected_root.map(Path::to_path_buf),
        anchor_lat_bucket: (scene_anchor.lat * 20.0).round() as i32,
        anchor_lon_bucket: (scene_anchor.lon * 20.0).round() as i32,
        zoom_bucket: (zoom * 10.0).round() as i32,
    };

    if guard.scene_key.as_ref() != Some(&scene_key) {
        guard.scene_key = Some(scene_key);
        guard.retained_keys.clear();
        guard.entries.clear();
    }

    let mut seen = guard.retained_keys.iter().cloned().collect::<HashSet<_>>();
    for asset in &assets {
        let key = CacheKey {
            path: asset.path.clone(),
            lat_bucket: asset.lat_bucket,
            lon_bucket: asset.lon_bucket,
            zoom_bucket: asset.zoom_bucket,
        };
        if seen.insert(key.clone()) {
            guard.retained_keys.push(key);
        }
    }

    let retained_keys = guard.retained_keys.clone();
    let mut merged = Vec::new();
    for key in &retained_keys {
        let contours = if let Some(cached) = guard.entries.get(key) {
            Arc::clone(cached)
        } else {
            let asset = assets.iter().find(|asset| {
                asset.path == key.path
                    && asset.zoom_bucket == key.zoom_bucket
                    && asset.lat_bucket == key.lat_bucket
                    && asset.lon_bucket == key.lon_bucket
            })?;
            let contours = Arc::new(
                query_local_contours(
                    &key.path,
                    key.zoom_bucket,
                    key.lat_bucket,
                    key.lon_bucket,
                    asset.simplify_step,
                    per_asset_budget,
                )
                .ok()?,
            );
            guard.entries.insert(key.clone(), Arc::clone(&contours));
            contours
        };
        merged.extend(contours.iter().cloned());
    }

    if merged.is_empty() {
        return None;
    }

    Some(Arc::new(merged))
}

pub fn load_for_focus(
    selected_root: Option<&Path>,
    focus: GeoPoint,
    zoom: f32,
) -> Option<Arc<Vec<ContourPath>>> {
    let path = contour_path(selected_root, zoom)?;
    let key = CacheKey {
        path,
        lat_bucket: (focus.lat * 2.0).round() as i32,
        lon_bucket: (focus.lon * 2.0).round() as i32,
        zoom_bucket: (zoom * 10.0).round() as i32,
    };

    static CACHE: OnceLock<Mutex<Option<CachedContours>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(None));
    let mut guard = cache.lock().ok()?;

    let needs_reload = guard
        .as_ref()
        .map(|cached| {
            cached.key.path != key.path
                || cached.key.lat_bucket != key.lat_bucket
                || cached.key.lon_bucket != key.lon_bucket
                || cached.key.zoom_bucket != key.zoom_bucket
        })
        .unwrap_or(true);

    if needs_reload {
        let contours = Arc::new(query_gebco_contours(&key.path, focus, zoom).ok()?);
        *guard = Some(CachedContours { key, contours });
    }

    guard.as_ref().map(|cached| Arc::clone(&cached.contours))
}

fn contour_path(selected_root: Option<&Path>, zoom: f32) -> Option<PathBuf> {
    let derived_root = terrain_assets::find_derived_root(selected_root)?;
    let file = if zoom >= 4.0 {
        "terrain/gebco_2025_contours_200m.gpkg"
    } else {
        "terrain/gebco_2025_contours_500m.gpkg"
    };

    let path = derived_root.join(file);
    path.exists().then_some(path)
}

fn query_gebco_contours(
    path: &Path,
    focus: GeoPoint,
    zoom: f32,
) -> rusqlite::Result<Vec<ContourPath>> {
    let connection = Connection::open(path)?;
    let half_extent = if zoom < 1.0 {
        8.0
    } else if zoom < 2.5 {
        4.0
    } else if zoom < 5.0 {
        2.25
    } else {
        1.2
    };
    let bounds = GeoBounds {
        min_lon: focus.lon - half_extent,
        max_lon: focus.lon + half_extent,
        min_lat: (focus.lat - half_extent).max(-90.0),
        max_lat: (focus.lat + half_extent).min(90.0),
    };
    let limit = if zoom >= 5.0 {
        180
    } else if zoom >= 2.5 {
        120
    } else {
        80
    };
    let simplify_step = if zoom >= 5.0 {
        2
    } else if zoom >= 2.5 {
        3
    } else {
        5
    };

    let mut statement = connection.prepare(
        "SELECT c.geom, c.elevation_m
         FROM contour c
         JOIN rtree_contour_geom r ON c.fid = r.id
         WHERE r.maxx >= ?1 AND r.minx <= ?2 AND r.maxy >= ?3 AND r.miny <= ?4
         LIMIT ?5",
    )?;

    let rows = statement.query_map(
        params![
            bounds.min_lon,
            bounds.max_lon,
            bounds.min_lat,
            bounds.max_lat,
            limit
        ],
        |row| {
            let geometry: Vec<u8> = row.get(0)?;
            let elevation_m: f32 = row.get(1)?;
            Ok((geometry, elevation_m))
        },
    )?;

    let mut contours = Vec::new();
    for row in rows {
        let (geometry, elevation_m) = row?;
        for line in parse_gpkg_lines(&geometry) {
            for clipped in clip_polyline_to_bounds(&line, bounds) {
                if clipped.len() < 2 {
                    continue;
                }
                contours.push(ContourPath {
                    elevation_m,
                    points: simplify_line(clipped, simplify_step),
                });
            }
        }
    }

    let positive_count = contours
        .iter()
        .filter(|contour| contour.elevation_m >= 0.0)
        .count();
    if positive_count >= contours.len().saturating_div(6).max(24) {
        contours.retain(|contour| contour.elevation_m >= 0.0);
    }

    Ok(contours)
}

fn query_local_contours(
    path: &Path,
    zoom_bucket: i32,
    lat_bucket: i32,
    lon_bucket: i32,
    simplify_step: usize,
    feature_budget: usize,
) -> rusqlite::Result<Vec<ContourPath>> {
    let connection = Connection::open(path)?;
    let mut statement = connection.prepare(
        "SELECT geom, elevation_m
         FROM contour_tiles
         WHERE zoom_bucket = ?1 AND lat_bucket = ?2 AND lon_bucket = ?3
         ORDER BY ABS(elevation_m), fid",
    )?;
    let rows = statement.query_map(params![zoom_bucket, lat_bucket, lon_bucket], |row| {
        let geometry: Vec<u8> = row.get(0)?;
        let elevation_m: f32 = row.get(1)?;
        Ok((geometry, elevation_m))
    })?;

    let mut contours = Vec::new();
    for row in rows {
        let (geometry, elevation_m) = row?;
        for line in parse_gpkg_lines(&geometry) {
            if line.len() < 2 {
                continue;
            }
            contours.push(ContourPath {
                elevation_m,
                points: simplify_line(line, simplify_step),
            });
        }
    }

    if contours.len() > feature_budget {
        let keep_step = contours.len().div_ceil(feature_budget.max(1));
        contours = contours
            .into_iter()
            .enumerate()
            .filter_map(|(index, contour)| (index % keep_step == 0).then_some(contour))
            .collect();
    }

    Ok(contours)
}

fn simplify_line(points: Vec<GeoPoint>, step: usize) -> Vec<GeoPoint> {
    if points.len() <= 2 || step <= 1 {
        return points;
    }

    let mut simplified: Vec<_> = points
        .iter()
        .enumerate()
        .filter_map(|(index, point)| {
            (index == 0 || index + 1 == points.len() || index % step == 0).then_some(*point)
        })
        .collect();

    simplified.dedup_by(|left, right| left.lat == right.lat && left.lon == right.lon);
    simplified
}

fn clip_polyline_to_bounds(points: &[GeoPoint], bounds: GeoBounds) -> Vec<Vec<GeoPoint>> {
    let mut result = Vec::new();
    let mut current = Vec::new();

    for pair in points.windows(2) {
        let start = pair[0];
        let end = pair[1];
        if let Some((clipped_start, clipped_end)) = clip_segment(start, end, bounds) {
            if current
                .last()
                .is_none_or(|last: &GeoPoint| points_distinct(*last, clipped_start))
            {
                current.push(clipped_start);
            }
            current.push(clipped_end);
        } else if current.len() >= 2 {
            result.push(std::mem::take(&mut current));
        } else {
            current.clear();
        }
    }

    if current.len() >= 2 {
        result.push(current);
    }

    result
}

fn clip_segment(start: GeoPoint, end: GeoPoint, bounds: GeoBounds) -> Option<(GeoPoint, GeoPoint)> {
    let mut t0 = 0.0f32;
    let mut t1 = 1.0f32;
    let dx = end.lon - start.lon;
    let dy = end.lat - start.lat;

    for (p, q) in [
        (-dx, start.lon - bounds.min_lon),
        (dx, bounds.max_lon - start.lon),
        (-dy, start.lat - bounds.min_lat),
        (dy, bounds.max_lat - start.lat),
    ] {
        if p.abs() <= f32::EPSILON {
            if q < 0.0 {
                return None;
            }
            continue;
        }

        let r = q / p;
        if p < 0.0 {
            if r > t1 {
                return None;
            }
            t0 = t0.max(r);
        } else {
            if r < t0 {
                return None;
            }
            t1 = t1.min(r);
        }
    }

    Some((
        GeoPoint {
            lat: start.lat + dy * t0,
            lon: start.lon + dx * t0,
        },
        GeoPoint {
            lat: start.lat + dy * t1,
            lon: start.lon + dx * t1,
        },
    ))
}

fn parse_gpkg_lines(blob: &[u8]) -> Vec<Vec<GeoPoint>> {
    if blob.len() < 8 || &blob[0..2] != b"GP" {
        return Vec::new();
    }

    let flags = blob[3];
    let envelope_indicator = (flags >> 1) & 0b111;
    let envelope_len = match envelope_indicator {
        0 => 0,
        1 => 32,
        2 | 3 => 48,
        4 => 64,
        _ => 0,
    };
    let header_len = 8 + envelope_len;
    if blob.len() <= header_len {
        return Vec::new();
    }

    parse_wkb_geometry(&blob[header_len..]).unwrap_or_default()
}

fn parse_wkb_geometry(wkb: &[u8]) -> Option<Vec<Vec<GeoPoint>>> {
    let mut cursor = 0usize;
    let endian = *wkb.get(cursor)?;
    cursor += 1;
    let little = endian == 1;
    let geom_type = read_u32(wkb, &mut cursor, little)?;
    let base_type = geom_type % 1000;

    match base_type {
        2 => Some(vec![parse_linestring(wkb, &mut cursor, little)?]),
        5 => {
            let count = read_u32(wkb, &mut cursor, little)? as usize;
            let mut lines = Vec::with_capacity(count);
            for _ in 0..count {
                let sub_geometry = parse_wkb_geometry(&wkb[cursor..])?;
                let consumed = consumed_geometry_bytes(&wkb[cursor..])?;
                cursor += consumed;
                lines.extend(sub_geometry);
            }
            Some(lines)
        }
        _ => None,
    }
}

fn consumed_geometry_bytes(wkb: &[u8]) -> Option<usize> {
    let mut cursor = 0usize;
    let endian = *wkb.get(cursor)?;
    cursor += 1;
    let little = endian == 1;
    let geom_type = read_u32(wkb, &mut cursor, little)?;
    let base_type = geom_type % 1000;

    match base_type {
        2 => {
            let count = read_u32(wkb, &mut cursor, little)? as usize;
            cursor += count * 16;
            Some(cursor)
        }
        5 => {
            let count = read_u32(wkb, &mut cursor, little)? as usize;
            for _ in 0..count {
                let consumed = consumed_geometry_bytes(&wkb[cursor..])?;
                cursor += consumed;
            }
            Some(cursor)
        }
        _ => None,
    }
}

fn parse_linestring(wkb: &[u8], cursor: &mut usize, little: bool) -> Option<Vec<GeoPoint>> {
    let count = read_u32(wkb, cursor, little)? as usize;
    let mut points = Vec::with_capacity(count);
    for _ in 0..count {
        let lon = read_f64(wkb, cursor, little)? as f32;
        let lat = read_f64(wkb, cursor, little)? as f32;
        points.push(GeoPoint { lat, lon });
    }
    Some(points)
}

fn read_u32(bytes: &[u8], cursor: &mut usize, little: bool) -> Option<u32> {
    let slice = bytes.get(*cursor..(*cursor + 4))?;
    *cursor += 4;
    Some(if little {
        u32::from_le_bytes(slice.try_into().ok()?)
    } else {
        u32::from_be_bytes(slice.try_into().ok()?)
    })
}

fn read_f64(bytes: &[u8], cursor: &mut usize, little: bool) -> Option<f64> {
    let slice = bytes.get(*cursor..(*cursor + 8))?;
    *cursor += 8;
    Some(if little {
        f64::from_le_bytes(slice.try_into().ok()?)
    } else {
        f64::from_be_bytes(slice.try_into().ok()?)
    })
}

fn points_distinct(left: GeoPoint, right: GeoPoint) -> bool {
    (left.lat - right.lat).abs() > 0.000_01 || (left.lon - right.lon).abs() > 0.000_01
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_cached_sqlite_focus_contours() {
        let path = Path::new("Derived/terrain/srtm_focus_cache.sqlite");
        if !path.exists() {
            return;
        }

        let connection = Connection::open(path).expect("should open shared SRTM cache DB");
        let tile = connection
            .query_row(
                "SELECT zoom_bucket, lat_bucket, lon_bucket
                 FROM contour_tile_manifest
                 LIMIT 1",
                [],
                |row| {
                    Ok((
                        row.get::<_, i32>(0)?,
                        row.get::<_, i32>(1)?,
                        row.get::<_, i32>(2)?,
                    ))
                },
            )
            .optional()
            .expect("manifest lookup should succeed");
        let Some((zoom_bucket, lat_bucket, lon_bucket)) = tile else {
            return;
        };

        let contours = query_local_contours(path, zoom_bucket, lat_bucket, lon_bucket, 2, 1_500)
            .expect("should read cached SRTM focus contours");
        assert!(
            !contours.is_empty(),
            "expected parsed contours from shared SQLite cache"
        );
        assert!(
            contours.iter().any(|contour| contour.points.len() >= 2),
            "expected visible polyline geometry"
        );
    }
}
