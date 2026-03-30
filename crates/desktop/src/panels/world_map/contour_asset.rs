use crate::model::GeoPoint;
use crate::terrain_assets;
use rusqlite::{Connection, params};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use super::gebco_depth_fill;
use super::srtm_focus_cache;

// ── Module-level cache statics ────────────────────────────────────────────────
// Lifted to module scope so blast_tile_caches() can clear them all at once.
static LOCAL_CONTOUR_CACHE: OnceLock<Mutex<LocalRegionCache>> = OnceLock::new();
static GLOBE_CONTOUR_CACHE: OnceLock<Mutex<GlobeRegionCache>> = OnceLock::new();
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
        if let Ok(mut g) = c.lock() {
            g.scene_key = None;
            g.entries.clear();
            g.in_flight.clear();
        }
    }
    if let Some(c) = GLOBE_CONTOUR_CACHE.get() {
        if let Ok(mut g) = c.lock() {
            *g = GlobeRegionCache::default();
        }
    }
    if let Some(c) = GLOBAL_COASTLINE_CACHE.get() {
        if let Ok(mut g) = c.lock() {
            *g = None;
        }
    }
    if let Some(c) = GLOBAL_TOPO_CACHE.get() {
        if let Ok(mut g) = c.lock() {
            *g = None;
        }
    }
    if let Some(c) = GLOBAL_BATHYMETRY_CACHE.get() {
        if let Ok(mut g) = c.lock() {
            *g = None;
        }
    }
    gebco_depth_fill::clear();
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
    /// Keys for which a background load thread has been spawned but not yet
    /// completed.  Prevents spawning O(N) duplicate threads per repaint
    /// (each finishing thread calls ctx.request_repaint(), which would
    /// otherwise trigger another batch of thread spawns for still-loading tiles).
    in_flight: HashSet<CacheKey>,
}

#[derive(Clone, PartialEq, Eq)]
struct SceneKey {
    root: Option<PathBuf>,
    anchor_lat_bucket: i32,
    anchor_lon_bucket: i32,
    zoom_bucket: i32,
}

pub fn load_srtm_region_for_view(
    selected_root: Option<&Path>,
    scene_anchor: GeoPoint,
    viewport_center: GeoPoint,
    zoom: f32,
    radius: i32,
    ctx: egui::Context,
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

    let cache: &'static Mutex<LocalRegionCache> = LOCAL_CONTOUR_CACHE.get_or_init(|| {
        Mutex::new(LocalRegionCache {
            scene_key: None,
            entries: HashMap::new(),
            in_flight: HashSet::new(),
        })
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

    // Phase 1: lock, check for scene change, collect tiles that need loading,
    // and atomically mark them as in-flight — all under a single lock
    // acquisition so no other frame can race and spawn duplicate threads.
    let missing: Vec<(CacheKey, srtm_focus_cache::FocusContourAsset)> = {
        let mut guard = cache.lock().ok()?;
        if guard.scene_key.as_ref() != Some(&scene_key) {
            eprintln!(
                "[1kEE] scene change → {} assets for zoom_bucket={} (entries cleared)",
                assets.len(),
                assets.first().map(|a| a.zoom_bucket).unwrap_or(-1)
            );
            guard.scene_key = Some(scene_key);
            guard.entries.clear();
            guard.in_flight.clear();
        }
        let missing: Vec<_> = assets
            .iter()
            .filter_map(|asset| {
                let key = CacheKey {
                    path: asset.path.clone(),
                    lat_bucket: asset.lat_bucket,
                    lon_bucket: asset.lon_bucket,
                    zoom_bucket: asset.zoom_bucket,
                };
                // Skip tiles already loaded or already being loaded by another thread.
                // Without this check, every repaint (including those triggered by
                // finishing threads) would spawn a fresh batch of N threads for the
                // still-loading tiles — an O(N²) explosion that looks like a strobe.
                if guard.entries.contains_key(&key) || guard.in_flight.contains(&key) {
                    None
                } else {
                    Some((key, asset.clone()))
                }
            })
            .collect();
        // Mark new tiles as in-flight before dropping the lock so the next
        // frame's Phase 1 won't double-spawn them.
        for (key, _) in &missing {
            guard.in_flight.insert(key.clone());
        }
        missing
    }; // guard dropped here

    // Phase 2: spawn detached threads for each newly-claimed tile; return
    // immediately with whatever is currently cached rather than blocking.
    for (key, asset) in missing {
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let query_result = query_local_contours(
                &key.path,
                key.zoom_bucket,
                key.lat_bucket,
                key.lon_bucket,
                asset.simplify_step,
                per_asset_budget,
            );
            // Debug: log result so it's visible in Console.app / Terminal stderr.
            match &query_result {
                Ok(contours) if contours.is_empty() => {
                    eprintln!(
                        "[1kEE] contour tile z{} ({},{}) → 0 lines (empty tile or nodata)",
                        key.zoom_bucket, key.lat_bucket, key.lon_bucket
                    );
                }
                Ok(contours) => {
                    eprintln!(
                        "[1kEE] contour tile z{} ({},{}) → {} lines OK",
                        key.zoom_bucket, key.lat_bucket, key.lon_bucket, contours.len()
                    );
                }
                Err(e) => {
                    eprintln!(
                        "[1kEE] contour tile z{} ({},{}) → ERROR: {e}",
                        key.zoom_bucket, key.lat_bucket, key.lon_bucket
                    );
                }
            }
            let result = query_result.ok().filter(|c| !c.is_empty());

            if let Ok(mut g) = cache.lock() {
                if let Some(contours) = result {
                    g.entries
                        .entry(key.clone())
                        .or_insert_with(|| Arc::new(contours));
                }
                // Always clear in-flight so a failed tile can be retried next frame.
                g.in_flight.remove(&key);
            }
            ctx.request_repaint();
        });
    }

    // Re-acquire lock to render whatever is currently cached.
    let mut guard = cache.lock().ok()?;

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
    ctx: egui::Context,
) -> Option<Arc<Vec<ContourPath>>> {
    const MAX_TILES: usize = 1600;
    // Use a fixed coarse zoom spec for globe-scale tiles (zoom_bucket=1,
    // half_extent=2.2°, ~244 km per side).  This keeps tile geographic size
    // constant as the actual view zoom changes — tiles don't shrink as the
    // globe grows.  radius=2 gives a 5×5 grid covering ~8.4° across.
    const GLOBE_TILE_ZOOM: f32 = 1.5;

    let assets =
        srtm_focus_cache::ensure_focus_contour_region(selected_root, center, GLOBE_TILE_ZOOM, 2);

    let cache: &'static Mutex<GlobeRegionCache> =
        GLOBE_CONTOUR_CACHE.get_or_init(|| Mutex::new(GlobeRegionCache::default()));
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

    // Spawn detached threads for each missing tile; return immediately with
    // whatever is currently cached rather than blocking the render thread.
    for asset in missing {
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            if let Some(contours) = query_local_contours(
                &asset.path,
                asset.zoom_bucket,
                asset.lat_bucket,
                asset.lon_bucket,
                asset.simplify_step,
                per_asset_budget,
            )
            .ok()
            .filter(|c| !c.is_empty())
            {
                if let Ok(mut g) = cache.lock() {
                    let key = (asset.lat_bucket, asset.lon_bucket);
                    if !g.tiles.contains_key(&key) {
                        g.tiles.insert(key, Arc::new(contours));
                        g.order.push(key);
                    }
                }
            }
            ctx.request_repaint();
        });
    }

    // Re-acquire lock to render and evict from whatever is currently cached.
    let mut guard = cache.lock().ok()?;

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

pub fn load_global_coastlines(
    selected_root: Option<&Path>,
    zoom: f32,
    ctx: egui::Context,
) -> Option<Arc<Vec<ContourPath>>> {
    let path = srtm_focus_cache::ensure_global_coastline_cache(selected_root).or_else(|| {
        let path = terrain_assets::find_derived_root(selected_root)?
            .join("terrain/gebco_2025_coastline_0m.gpkg");
        path.exists().then_some(path)
    })?;
    let (lod_bucket, simplify_step, feature_budget) = global_coastline_lod(zoom);

    let cache: &'static Mutex<Option<CachedGlobalContours>> =
        GLOBAL_COASTLINE_CACHE.get_or_init(|| Mutex::new(None));
    let guard = cache.lock().ok()?;

    let needs_reload = guard
        .as_ref()
        .map(|cached| cached.path.as_path() != path.as_path() || cached.lod_bucket != lod_bucket)
        .unwrap_or(true);

    if needs_reload {
        let old_result = guard.as_ref().map(|c| Arc::clone(&c.contours));
        drop(guard);
        static LOADING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if !LOADING.swap(true, std::sync::atomic::Ordering::SeqCst) {
            let path_bg = path.clone();
            std::thread::spawn(move || {
                if let Ok(contours) =
                    query_global_coastlines(&path_bg, simplify_step, feature_budget)
                {
                    if let Ok(mut g) = cache.lock() {
                        *g = Some(CachedGlobalContours {
                            lod_bucket,
                            path: path_bg,
                            contours: Arc::new(contours),
                        });
                    }
                }
                LOADING.store(false, std::sync::atomic::Ordering::SeqCst);
                ctx.request_repaint();
            });
        }
        return old_result;
    }

    guard.as_ref().map(|cached| Arc::clone(&cached.contours))
}

pub fn global_coastlines_pending(_selected_root: Option<&Path>) -> bool {
    srtm_focus_cache::is_global_coastline_building()
}

pub fn load_global_topo(
    selected_root: Option<&Path>,
    zoom: f32,
    ctx: egui::Context,
) -> Option<Arc<Vec<ContourPath>>> {
    // Triggers a one-time background GDAL build from available SRTM tiles if
    // the file doesn't yet exist.  Returns None while the build is in progress.
    let path = srtm_focus_cache::ensure_global_land_overview(selected_root)?;
    let (lod_bucket, simplify_step, feature_budget) = global_topo_lod(zoom);

    let cache: &'static Mutex<Option<CachedGlobalContours>> =
        GLOBAL_TOPO_CACHE.get_or_init(|| Mutex::new(None));
    let guard = cache.lock().ok()?;

    let needs_reload = guard
        .as_ref()
        .map(|cached| cached.path.as_path() != path.as_path() || cached.lod_bucket != lod_bucket)
        .unwrap_or(true);

    if needs_reload {
        let old_result = guard.as_ref().map(|c| Arc::clone(&c.contours));
        drop(guard);
        static LOADING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if !LOADING.swap(true, std::sync::atomic::Ordering::SeqCst) {
            let path_bg = path.clone();
            std::thread::spawn(move || {
                if let Ok(contours) = query_global_topo(&path_bg, simplify_step, feature_budget) {
                    if let Ok(mut g) = cache.lock() {
                        *g = Some(CachedGlobalContours {
                            lod_bucket,
                            path: path_bg,
                            contours: Arc::new(contours),
                        });
                    }
                }
                LOADING.store(false, std::sync::atomic::Ordering::SeqCst);
                ctx.request_repaint();
            });
        }
        return old_result;
    }

    guard.as_ref().map(|cached| Arc::clone(&cached.contours))
}

pub fn load_global_bathymetry(
    selected_root: Option<&Path>,
    zoom: f32,
    ctx: egui::Context,
) -> Option<Arc<Vec<ContourPath>>> {
    // Trigger background generation of derived GEBCO assets when missing.
    // This is a no-op once both files exist and is cheap to call every frame.
    srtm_focus_cache::ensure_gebco_derived(selected_root);
    let path = contour_path(selected_root, zoom)?;
    // Single LOD — no zoom-based switching so the cache never reloads on zoom
    // changes (which was causing contours to appear/disappear while panning).
    let lod_bucket: i32 = 0;
    let simplify_step: usize = 4;
    let feature_budget: usize = 4_000;

    let cache: &'static Mutex<Option<CachedGlobalContours>> =
        GLOBAL_BATHYMETRY_CACHE.get_or_init(|| Mutex::new(None));
    let guard = cache.lock().ok()?;

    let needs_reload = guard
        .as_ref()
        .map(|cached| cached.path.as_path() != path.as_path() || cached.lod_bucket != lod_bucket)
        .unwrap_or(true);

    if needs_reload {
        let old_result = guard.as_ref().map(|c| Arc::clone(&c.contours));
        drop(guard);
        static LOADING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if !LOADING.swap(true, std::sync::atomic::Ordering::SeqCst) {
            let path_bg = path.clone();
            std::thread::spawn(move || {
                if let Ok(contours) =
                    query_global_bathymetry(&path_bg, simplify_step, feature_budget)
                {
                    if let Ok(mut g) = cache.lock() {
                        *g = Some(CachedGlobalContours {
                            lod_bucket,
                            path: path_bg,
                            contours: Arc::new(contours),
                        });
                    }
                }
                LOADING.store(false, std::sync::atomic::Ordering::SeqCst);
                ctx.request_repaint();
            });
        }
        return old_result;
    }

    guard.as_ref().map(|cached| Arc::clone(&cached.contours))
}

fn query_global_bathymetry(
    path: &Path,
    simplify_step: usize,
    feature_budget: usize,
) -> rusqlite::Result<Vec<ContourPath>> {
    let connection = Connection::open(path)?;

    // gdal_contour fragments each isobath into many short scan-line segments.
    // Stride-sampling those fragments gives a globally-distributed but spotty
    // point cloud.  Instead, query a set of geomorphologically meaningful
    // depth levels — continental shelf edge (-200m), slope, abyssal plain
    // transitions — fetch ALL fragments at each level, sort by length, and
    // keep the longest ones.  Long fragments = major shelf/ridge/trench lines.
    //
    // Budget split across depth levels (shelf gets the most):
    let depth_levels: &[(f32, usize)] = &[
        (-200.0, feature_budget * 22 / 100),  // continental shelf edge
        (-500.0, feature_budget * 12 / 100),  // upper slope
        (-1000.0, feature_budget * 12 / 100), // mid slope
        (-2000.0, feature_budget * 16 / 100), // ridge crests / lower slope
        (-3000.0, feature_budget * 14 / 100), // ridge flanks / abyssal rise
        (-4000.0, feature_budget * 12 / 100), // deep abyssal plains
        (-5000.0, feature_budget * 8 / 100),  // hadal zone entry
        (-6000.0, feature_budget * 4 / 100),  // trenches
    ];

    let mut all_contours: Vec<ContourPath> = Vec::new();

    for &(depth, per_depth_budget) in depth_levels {
        if per_depth_budget == 0 {
            continue;
        }
        // Let SQLite do the global sort by raw byte length so we get the
        // geographically significant features (mid-Atlantic ridge, shelf edges,
        // trench walls) regardless of where in the table they live.
        // Previous approach used LIMIT N without ORDER BY, which returned only
        // the northernmost rows (N→S scan order) and missed major S-hemisphere
        // features entirely.  Full-table scan here costs ~100 ms/depth level
        // total — acceptable for a one-time startup load cached in a Mutex.
        let budget = per_depth_budget as i64;
        let mut stmt = connection.prepare(
            "SELECT geom FROM contour \
             WHERE ABS(elevation_m - ?1) < 1.0 \
             ORDER BY length(geom) DESC \
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![depth, budget], |row| row.get::<_, Vec<u8>>(0))?;

        for row in rows {
            let geometry = row?;
            for line in parse_gpkg_lines(&geometry) {
                if line.len() < 2 {
                    continue;
                }
                let simplified = simplify_line(line, simplify_step);
                if simplified.len() >= 3 {
                    all_contours.push(ContourPath {
                        elevation_m: depth,
                        points: simplified,
                    });
                }
            }
        }
    }

    Ok(all_contours)
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
    // Land-positive contours only.  The GPKG is ordered by scan position (N→S)
    // so a bare LIMIT returns only Arctic/northern features.  Stride-sample
    // across the full FID range to get globally-distributed land contours.
    let total: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM contour WHERE elevation_m > 0",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let fetch_target = (feature_budget * 8).max(3_000) as i64;
    let stride = (total / fetch_target.max(1)).max(1);
    let mut statement = connection.prepare(
        "SELECT geom, elevation_m FROM contour WHERE elevation_m > 0 AND (fid % ?1) = 0",
    )?;
    let rows = statement.query_map(params![stride], |row| {
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

fn contour_path(selected_root: Option<&Path>, _zoom: f32) -> Option<PathBuf> {
    let derived_root = terrain_assets::find_derived_root(selected_root)?;
    // Only the 200m GPKG exists; use it at all zoom levels.
    let path = derived_root.join("terrain/gebco_2025_contours_200m.gpkg");
    path.exists().then_some(path)
}

fn global_coastline_lod(_zoom: f32) -> (i32, usize, usize) {
    // Single LOD bucket so the cache never reloads on zoom change (changing
    // the bucket was causing full-table-scan reloads = visible flicker).
    // Budget/step are fixed at a mid-range density: comparable to the old
    // zoom-1.8–3.5 tier which looked correct on the globe.
    (0, 5, 2_500)
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
