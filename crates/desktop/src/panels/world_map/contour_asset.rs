use crate::model::GeoPoint;
use crate::terrain_assets;
use rusqlite::{Connection, params};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use super::srtm_focus_cache;

// ── Module-level cache statics ────────────────────────────────────────────────
// Lifted to module scope so blast_tile_caches() can clear them all at once.
static LOCAL_CONTOUR_CACHE: OnceLock<Mutex<LocalRegionCache>> = OnceLock::new();
static LOCAL_COASTLINE_CACHE: OnceLock<Mutex<LocalRegionCache>> = OnceLock::new();
static GLOBE_CONTOUR_CACHE: OnceLock<Mutex<GlobeRegionCache>> = OnceLock::new();
static FOCUS_CONTOUR_CACHE: OnceLock<Mutex<Option<CachedContours>>> = OnceLock::new();
static GLOBAL_COASTLINE_CACHE: OnceLock<Mutex<Option<CachedGlobalContours>>> = OnceLock::new();
static GLOBAL_TOPO_CACHE: OnceLock<Mutex<Option<CachedGlobalContours>>> = OnceLock::new();
static GLOBAL_BATHYMETRY_CACHE: OnceLock<Mutex<Option<CachedGlobalContours>>> = OnceLock::new();

/// Instantly drop every in-memory tile cache.
///
/// Forces a full reload on the next frame — both the global globe view and the
/// local terrain view.  Does NOT delete anything from disk; the SQLite cache
/// files are untouched and tiles will be re-read (not re-built) on demand.
pub fn blast_tile_caches() {
    if let Some(c) = LOCAL_CONTOUR_CACHE.get() {
        if let Ok(mut g) = c.lock() { g.scene_key = None; g.entries.clear(); }
    }
    if let Some(c) = LOCAL_COASTLINE_CACHE.get() {
        if let Ok(mut g) = c.lock() { g.scene_key = None; g.entries.clear(); }
    }
    if let Some(c) = GLOBE_CONTOUR_CACHE.get() {
        if let Ok(mut g) = c.lock() { *g = GlobeRegionCache::default(); }
    }
    if let Some(c) = FOCUS_CONTOUR_CACHE.get() {
        if let Ok(mut g) = c.lock() { *g = None; }
    }
    if let Some(c) = GLOBAL_COASTLINE_CACHE.get() {
        if let Ok(mut g) = c.lock() { *g = None; }
    }
    if let Some(c) = GLOBAL_TOPO_CACHE.get() {
        if let Ok(mut g) = c.lock() { *g = None; }
    }
    if let Some(c) = GLOBAL_BATHYMETRY_CACHE.get() {
        if let Ok(mut g) = c.lock() { *g = None; }
    }
}

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

struct CachedGlobalContours {
    lod_bucket: i32,
    path: PathBuf,
    contours: Arc<Vec<ContourPath>>,
}

/// Per-zoom-level cache for globe-mode SRTM tiles.
/// Unlike `LocalRegionCache`, this accumulates tiles across orbit movements
/// and only clears when the zoom bucket changes.  Eviction is by distance
/// from the current center, so tiles stay visible while on screen.
struct GlobeRegionCache {
    zoom_bucket: i32,
    root: Option<PathBuf>,
    /// (lat_bucket, lon_bucket) → decoded contour paths
    tiles: HashMap<(i32, i32), Arc<Vec<ContourPath>>>,
    /// Insertion order for deterministic eviction among equal-distance ties
    order: Vec<(i32, i32)>,
}

impl Default for GlobeRegionCache {
    fn default() -> Self {
        Self {
            zoom_bucket: -1,
            root: None,
            tiles: HashMap::new(),
            order: Vec::new(),
        }
    }
}

struct LocalRegionCache {
    scene_key: Option<SceneKey>,
    entries: HashMap<CacheKey, Arc<Vec<ContourPath>>>,
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

    // Maximum number of tiles to keep in the local-terrain cache.
    // Each tile holds up to ~120 contour paths, so 200 tiles ≈ 24 000 paths
    // — well within real-time render budget.
    const MAX_LOCAL_TILES: usize = 200;

    let cache = LOCAL_CONTOUR_CACHE.get_or_init(|| {
        Mutex::new(LocalRegionCache { scene_key: None, entries: HashMap::new() })
    });
    let feature_budget = srtm_focus_cache::feature_budget_for_zoom(zoom);
    let per_asset_budget = (feature_budget / assets.len().max(1)).max(120);
    let scene_key = SceneKey {
        root: selected_root.map(Path::to_path_buf),
        anchor_lat_bucket: (scene_anchor.lat * 20.0).round() as i32,
        anchor_lon_bucket: (scene_anchor.lon * 20.0).round() as i32,
        zoom_bucket: assets
            .first()
            .map(|asset| asset.zoom_bucket)
            .unwrap_or_default(),
    };

    // Phase 1: lock, check for scene change, collect missing keys, then drop lock
    // so the DB reads can run in parallel without blocking the render thread on
    // the mutex.
    let missing: Vec<(CacheKey, srtm_focus_cache::FocusContourAsset)> = {
        let mut guard = cache.lock().ok()?;
        if guard.scene_key.as_ref() != Some(&scene_key) {
            guard.scene_key = Some(scene_key);
            guard.entries.clear();
        }
        assets
            .iter()
            .filter_map(|asset| {
                let key = CacheKey {
                    path: asset.path.clone(),
                    lat_bucket: asset.lat_bucket,
                    lon_bucket: asset.lon_bucket,
                    zoom_bucket: asset.zoom_bucket,
                };
                if guard.entries.contains_key(&key) {
                    None
                } else {
                    Some((key, asset.clone()))
                }
            })
            .collect()
    }; // guard dropped here

    // Phase 2: load all missing tiles in parallel — each opens its own DB
    // connection so SQLite concurrent-read is safe, and no mutex is held.
    let loaded: Vec<(CacheKey, Vec<ContourPath>)> = std::thread::scope(|s| {
        let handles: Vec<_> = missing
            .into_iter()
            .map(|(key, asset)| {
                s.spawn(move || {
                    query_local_contours(
                        &key.path,
                        key.zoom_bucket,
                        key.lat_bucket,
                        key.lon_bucket,
                        asset.simplify_step,
                        per_asset_budget,
                    )
                    .ok()
                    .filter(|c| !c.is_empty())
                    .map(|c| (key, c))
                })
            })
            .collect();
        handles
            .into_iter()
            .filter_map(|h| h.join().ok().flatten())
            .collect()
    });

    // Phase 3: re-acquire lock, insert results.
    let mut guard = cache.lock().ok()?;
    for (key, contours) in loaded {
        guard.entries.insert(key, Arc::new(contours));
    }

    // Evict tiles furthest from viewport_center when over the cap.
    if guard.entries.len() > MAX_LOCAL_TILES {
        let bucket_step = srtm_focus_cache::half_extent_for_zoom(zoom) * 0.45;
        let clat = (viewport_center.lat / bucket_step).round() as i32;
        let clon = (viewport_center.lon / bucket_step).round() as i32;

        let mut keys: Vec<CacheKey> = guard.entries.keys().cloned().collect();
        // Sort furthest-first so we can truncate from the back.
        keys.sort_unstable_by_key(|k| {
            let dlat = k.lat_bucket - clat;
            let dlon = k.lon_bucket - clon;
            -(dlat * dlat + dlon * dlon)
        });
        let excess = guard.entries.len() - MAX_LOCAL_TILES;
        for k in keys.into_iter().take(excess) {
            guard.entries.remove(&k);
        }
    }

    // Render ALL accumulated tiles, not just the current viewport grid.
    let mut merged = Vec::new();
    for contours in guard.entries.values() {
        merged.extend(contours.iter().cloned());
    }

    if merged.is_empty() {
        return None;
    }

    Some(Arc::new(merged))
}

/// Load SRTM-derived 0m coastline paths for the local terrain view.
///
/// Calls `ensure_focus_coastline_region` to trigger background builds for any
/// tiles that have main contour data but haven't had their coastline extracted
/// yet.  Returns `None` until at least one coastline tile is ready.
pub fn load_srtm_coastlines_for_view(
    selected_root: Option<&Path>,
    viewport_center: GeoPoint,
    zoom: f32,
    radius: i32,
) -> Option<Arc<Vec<ContourPath>>> {
    let assets = srtm_focus_cache::ensure_focus_coastline_region(
        selected_root, viewport_center, zoom, radius,
    );
    if assets.is_empty() {
        return None;
    }

    let cache = LOCAL_COASTLINE_CACHE.get_or_init(|| {
        Mutex::new(LocalRegionCache { scene_key: None, entries: HashMap::new() })
    });

    let scene_key = SceneKey {
        root: selected_root.map(Path::to_path_buf),
        anchor_lat_bucket: (viewport_center.lat * 20.0).round() as i32,
        anchor_lon_bucket: (viewport_center.lon * 20.0).round() as i32,
        zoom_bucket: assets.first().map(|a| a.zoom_bucket).unwrap_or_default(),
    };

    // Phase 1: lock, check scene, collect missing keys, drop lock immediately.
    // We never block the render thread waiting for DB reads.
    let missing: Vec<(CacheKey, srtm_focus_cache::FocusContourAsset)> = {
        let mut guard = cache.lock().ok()?;
        if guard.scene_key.as_ref() != Some(&scene_key) {
            guard.scene_key = Some(scene_key);
            guard.entries.clear();
        }
        assets
            .iter()
            .filter_map(|asset| {
                let key = CacheKey {
                    path: asset.path.clone(),
                    lat_bucket: asset.lat_bucket,
                    lon_bucket: asset.lon_bucket,
                    zoom_bucket: asset.zoom_bucket,
                };
                if guard.entries.contains_key(&key) { None } else { Some((key, asset.clone())) }
            })
            .collect()
    }; // lock dropped here — render thread is free

    // Phase 2: spawn detached threads for each missing tile.  They write into
    // the shared cache and request a repaint when done.  This call returns
    // immediately; the coastline appears on the next frame(s) as tiles load.
    // `cache` is `&'static`, so it satisfies the `'static` bound on thread::spawn.
    for (key, _asset) in missing {
        std::thread::spawn(move || {
            let result = query_local_coastlines(
                &key.path,
                key.zoom_bucket,
                key.lat_bucket,
                key.lon_bucket,
            )
            .ok()
            .filter(|c| !c.is_empty());
            if let Some(contours) = result {
                if let Ok(mut guard) = cache.lock() {
                    guard.entries.insert(key, Arc::new(contours));
                }
                crate::app::request_repaint();
            }
        });
    }

    // Phase 3: return whatever is already cached — may be empty on the first
    // call, populated on subsequent frames as the spawned threads complete.
    let guard = cache.lock().ok()?;
    let merged: Vec<ContourPath> = guard
        .entries
        .values()
        .flat_map(|v| v.iter().cloned())
        .collect();

    if merged.is_empty() { None } else { Some(Arc::new(merged)) }
}

/// Load SRTM focus-tile contours for globe-mode rendering.
///
/// Differences from `load_srtm_region_for_view`:
/// - Loads a 3×3 tile grid (radius=1) so neighbours are pre-fetched before
///   they scroll into view, preventing pop-in.
/// - Cache clears only on zoom-bucket change, not on position; tiles remain
///   visible while they are near the current centre.
/// - Evicts by distance from centre when the tile count exceeds `MAX_TILES`.
pub fn load_srtm_for_globe(
    selected_root: Option<&Path>,
    center: GeoPoint,
    _zoom: f32,
) -> Option<Arc<Vec<ContourPath>>> {
    const MAX_TILES: usize = 1600;
    // Use a fixed coarse zoom spec for globe-scale tiles (zoom_bucket=1,
    // half_extent=2.2°, ~244 km per side).  This keeps tile geographic size
    // constant as the actual view zoom changes — tiles don't shrink as the
    // globe grows.  radius=2 gives a 5×5 grid covering ~8.4° across.
    const GLOBE_TILE_ZOOM: f32 = 1.5;

    let assets =
        srtm_focus_cache::ensure_focus_contour_region(selected_root, center, GLOBE_TILE_ZOOM, 2);

    let cache = GLOBE_CONTOUR_CACHE.get_or_init(|| Mutex::new(GlobeRegionCache::default()));
    let mut guard = cache.lock().ok()?;

    let zoom_bucket = srtm_focus_cache::zoom_bucket_for_zoom(GLOBE_TILE_ZOOM);
    let root = selected_root.map(Path::to_path_buf);

    // Invalidate only on root change — zoom is now fixed so zoom_bucket never
    // changes, and position changes should accumulate rather than clear.
    if guard.zoom_bucket != zoom_bucket || guard.root != root {
        guard.zoom_bucket = zoom_bucket;
        guard.root = root;
        guard.tiles.clear();
        guard.order.clear();
    }

    if assets.is_empty() {
        // No SRTM root found; return whatever we already have.
        return render_globe_tiles(&guard);
    }

    let feature_budget = srtm_focus_cache::feature_budget_for_zoom(GLOBE_TILE_ZOOM);
    let per_asset_budget = (feature_budget / assets.len().max(1)).max(120);

    // Collect missing keys, then drop the lock before doing DB reads.
    let missing: Vec<srtm_focus_cache::FocusContourAsset> = assets
        .iter()
        .filter(|a| !guard.tiles.contains_key(&(a.lat_bucket, a.lon_bucket)))
        .cloned()
        .collect();
    drop(guard);

    // Load all missing globe tiles in parallel.
    let loaded: Vec<((i32, i32), Vec<ContourPath>)> = std::thread::scope(|s| {
        let handles: Vec<_> = missing
            .into_iter()
            .map(|asset| {
                s.spawn(move || {
                    query_local_contours(
                        &asset.path,
                        asset.zoom_bucket,
                        asset.lat_bucket,
                        asset.lon_bucket,
                        asset.simplify_step,
                        per_asset_budget,
                    )
                    .ok()
                    .filter(|c| !c.is_empty())
                    .map(|c| ((asset.lat_bucket, asset.lon_bucket), c))
                })
            })
            .collect();
        handles
            .into_iter()
            .filter_map(|h| h.join().ok().flatten())
            .collect()
    });

    let mut guard = cache.lock().ok()?;
    for (key, contours) in loaded {
        if !guard.tiles.contains_key(&key) {
            guard.tiles.insert(key, Arc::new(contours));
            guard.order.push(key);
        }
    }

    // Evict tiles furthest from centre when over the cap.
    if guard.tiles.len() > MAX_TILES {
        let half_extent = srtm_focus_cache::half_extent_for_zoom(GLOBE_TILE_ZOOM);
        let bucket_step = half_extent * 0.45;
        let clat = (center.lat / bucket_step).round() as i32;
        let clon = (center.lon / bucket_step).round() as i32;

        // Sort order vec by distance ascending; keep the closest MAX_TILES.
        guard
            .order
            .sort_by_key(|&(lat, lon)| (lat - clat).pow(2) + (lon - clon).pow(2));
        let keep: std::collections::HashSet<(i32, i32)> =
            guard.order[..MAX_TILES].iter().copied().collect();
        guard.tiles.retain(|k, _| keep.contains(k));
        guard.order.retain(|k| keep.contains(k));
    }

    render_globe_tiles(&guard)
}

fn render_globe_tiles(guard: &GlobeRegionCache) -> Option<Arc<Vec<ContourPath>>> {
    let merged: Vec<ContourPath> = guard
        .tiles
        .values()
        .flat_map(|v| v.iter().cloned())
        .collect();
    if merged.is_empty() {
        None
    } else {
        Some(Arc::new(merged))
    }
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

    let cache = FOCUS_CONTOUR_CACHE.get_or_init(|| Mutex::new(None));
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

pub fn load_global_coastlines(
    selected_root: Option<&Path>,
    zoom: f32,
) -> Option<Arc<Vec<ContourPath>>> {
    let path = srtm_focus_cache::ensure_global_coastline_cache(selected_root).or_else(|| {
        let path = terrain_assets::find_derived_root(selected_root)?
            .join("terrain/gebco_2025_coastline_0m.gpkg");
        path.exists().then_some(path)
    })?;
    let (lod_bucket, simplify_step, feature_budget) = global_coastline_lod(zoom);

    let cache = GLOBAL_COASTLINE_CACHE.get_or_init(|| Mutex::new(None));
    let mut guard = cache.lock().ok()?;

    let needs_reload = guard
        .as_ref()
        .map(|cached| cached.path.as_path() != path.as_path() || cached.lod_bucket != lod_bucket)
        .unwrap_or(true);

    if needs_reload {
        let contours =
            Arc::new(query_global_coastlines(&path, simplify_step, feature_budget).ok()?);
        *guard = Some(CachedGlobalContours {
            lod_bucket,
            path,
            contours,
        });
    }

    guard.as_ref().map(|cached| Arc::clone(&cached.contours))
}

pub fn global_coastlines_pending(_selected_root: Option<&Path>) -> bool {
    srtm_focus_cache::is_global_coastline_building()
}

pub fn load_global_topo(selected_root: Option<&Path>, zoom: f32) -> Option<Arc<Vec<ContourPath>>> {
    // Triggers a one-time background GDAL build from available SRTM tiles if
    // the file doesn't yet exist.  Returns None while the build is in progress.
    let path = srtm_focus_cache::ensure_global_land_overview(selected_root)?;
    let (lod_bucket, simplify_step, feature_budget) = global_topo_lod(zoom);

    let cache = GLOBAL_TOPO_CACHE.get_or_init(|| Mutex::new(None));
    let mut guard = cache.lock().ok()?;

    let needs_reload = guard
        .as_ref()
        .map(|cached| cached.path.as_path() != path.as_path() || cached.lod_bucket != lod_bucket)
        .unwrap_or(true);

    if needs_reload {
        let contours = Arc::new(query_global_topo(&path, simplify_step, feature_budget).ok()?);
        *guard = Some(CachedGlobalContours {
            lod_bucket,
            path,
            contours,
        });
    }

    guard.as_ref().map(|cached| Arc::clone(&cached.contours))
}

fn global_bathymetry_lod(zoom: f32) -> (i32, usize, usize) {
    if zoom < 1.5 {
        (0, 20, 300)
    } else if zoom < 3.0 {
        (1, 13, 550)
    } else {
        (2, 8, 900)
    }
}

pub fn load_global_bathymetry(
    selected_root: Option<&Path>,
    zoom: f32,
) -> Option<Arc<Vec<ContourPath>>> {
    let path = contour_path(selected_root, zoom)?;
    let (lod_bucket, simplify_step, feature_budget) = global_bathymetry_lod(zoom);

    let cache = GLOBAL_BATHYMETRY_CACHE.get_or_init(|| Mutex::new(None));
    let mut guard = cache.lock().ok()?;

    let needs_reload = guard
        .as_ref()
        .map(|cached| cached.path.as_path() != path.as_path() || cached.lod_bucket != lod_bucket)
        .unwrap_or(true);

    if needs_reload {
        let contours =
            Arc::new(query_global_bathymetry(&path, simplify_step, feature_budget).ok()?);
        *guard = Some(CachedGlobalContours { lod_bucket, path, contours });
    }

    guard.as_ref().map(|cached| Arc::clone(&cached.contours))
}

fn query_global_bathymetry(
    path: &Path,
    simplify_step: usize,
    feature_budget: usize,
) -> rusqlite::Result<Vec<ContourPath>> {
    let connection = Connection::open(path)?;
    // Fetch ocean contours only (negative elevation).
    // Prioritise mid-depth range (-200 to -6000m) where shelf/slope structure
    // is most visually interesting; include abyssal but at lower density.
    let fetch_limit = (feature_budget * 8).max(3_000) as i64;
    let mut statement = connection
        .prepare("SELECT geom, elevation_m FROM contour WHERE elevation_m < 0 LIMIT ?1")?;
    let rows = statement.query_map(params![fetch_limit], |row| {
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
            let simplified = simplify_line(line, simplify_step);
            if simplified.len() < 2 {
                continue;
            }
            contours.push(ContourPath { elevation_m, points: simplified });
        }
    }

    // Sort by line length descending — long lines are continental shelf edges,
    // ocean ridges, and trench walls; short ones are noise at this scale.
    contours.sort_unstable_by(|a, b| b.points.len().cmp(&a.points.len()));
    contours.truncate(feature_budget);
    Ok(contours)
}

fn global_topo_lod(zoom: f32) -> (i32, usize, usize) {
    if zoom < 1.5 {
        (0, 18, 400)
    } else if zoom < 3.0 {
        (1, 12, 650)
    } else {
        (2, 7, 1_000)
    }
}

fn query_global_topo(
    path: &Path,
    simplify_step: usize,
    feature_budget: usize,
) -> rusqlite::Result<Vec<ContourPath>> {
    let connection = Connection::open(path)?;
    // Land-positive contours only — skips ocean bathymetry which is visually
    // cluttered and tactically irrelevant at globe scale.
    // No ORDER BY: a bare LIMIT on the sequential scan is much faster than
    // sorting millions of rows in SQLite.  We sort by simplified line length
    // in Rust to keep geographically significant features (long ridges,
    // plateaus, continental edges) and drop short noise.
    let fetch_limit = (feature_budget * 8).max(3_000) as i64;
    let mut statement = connection
        .prepare("SELECT geom, elevation_m FROM contour WHERE elevation_m > 0 LIMIT ?1")?;
    let rows = statement.query_map(params![fetch_limit], |row| {
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
            let simplified = simplify_line(line, simplify_step);
            if simplified.len() < 2 {
                continue;
            }
            contours.push(ContourPath {
                elevation_m,
                points: simplified,
            });
        }
    }

    // Longest simplified lines are the most geographically prominent at globe scale.
    contours.sort_unstable_by(|a, b| b.points.len().cmp(&a.points.len()));
    contours.truncate(feature_budget);

    Ok(contours)
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

fn global_coastline_lod(zoom: f32) -> (i32, usize, usize) {
    if zoom < 0.95 {
        (0, 14, 700)
    } else if zoom < 1.8 {
        (1, 9, 1_300)
    } else if zoom < 3.5 {
        (2, 6, 2_400)
    } else if zoom < 6.0 {
        (3, 2, 6_000)
    } else {
        (4, 1, 10_000)
    }
}

fn query_global_coastlines(
    path: &Path,
    simplify_step: usize,
    feature_budget: usize,
) -> rusqlite::Result<Vec<ContourPath>> {
    let connection = Connection::open(path)?;
    let mut statement = connection.prepare("SELECT geom, elevation_m FROM contour ORDER BY fid")?;
    let rows = statement.query_map([], |row| {
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
            let simplified = simplify_line(line, simplify_step);
            if simplified.len() < 2 {
                continue;
            }
            contours.push(ContourPath {
                elevation_m,
                points: simplified,
            });
        }
    }

    if contours.len() > feature_budget {
        contours.sort_by(|left, right| right.points.len().cmp(&left.points.len()));
        contours.truncate(feature_budget);
    }

    Ok(contours)
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

fn query_local_coastlines(
    path: &Path,
    zoom_bucket: i32,
    lat_bucket: i32,
    lon_bucket: i32,
) -> rusqlite::Result<Vec<ContourPath>> {
    let connection = Connection::open(path)?;
    let mut statement = connection.prepare(
        "SELECT geom FROM coastline_tiles
         WHERE zoom_bucket = ?1 AND lat_bucket = ?2 AND lon_bucket = ?3
         ORDER BY fid",
    )?;
    let rows = statement.query_map(params![zoom_bucket, lat_bucket, lon_bucket], |row| {
        row.get::<_, Vec<u8>>(0)
    })?;

    let mut contours = Vec::new();
    for row in rows {
        let geometry = row?;
        for line in parse_gpkg_lines(&geometry) {
            // Drop very short fragments — 0m SRTM contours produce thousands of
            // tiny rings around inland sea-level pixels (river deltas, wetlands)
            // that render as noise dots.  Require at least 8 points (~240 m of
            // coastline at 30 m resolution) to keep only meaningful segments.
            if line.len() >= 8 {
                contours.push(ContourPath { elevation_m: 0.0, points: line });
            }
        }
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
