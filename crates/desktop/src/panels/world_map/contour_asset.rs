use crate::model::GeoPoint;
use crate::terrain_assets;
use rusqlite::params;
#[cfg(test)]
use rusqlite::Connection;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::gebco_depth_fill;
use super::srtm_focus_cache;

// ── Module-level cache statics ────────────────────────────────────────────────
// Lifted to module scope so blast_tile_caches() can clear them all at once.
static LOCAL_CONTOUR_CACHE: OnceLock<Mutex<LocalRegionCache>> = OnceLock::new();
static LUNAR_LOCAL_CONTOUR_CACHE: OnceLock<Mutex<LocalRegionCache>> = OnceLock::new();
static GLOBE_CONTOUR_CACHE: OnceLock<Mutex<GlobeRegionCache>> = OnceLock::new();
static LUNAR_GLOBE_CONTOUR_CACHE: OnceLock<Mutex<GlobeRegionCache>> = OnceLock::new();
static MARS_LOCAL_CONTOUR_CACHE: OnceLock<Mutex<LocalRegionCache>> = OnceLock::new();
static MARS_GLOBE_CONTOUR_CACHE: OnceLock<Mutex<GlobeRegionCache>> = OnceLock::new();
static GLOBAL_COASTLINE_CACHE: OnceLock<Mutex<Option<CachedGlobalContours>>> = OnceLock::new();
static GLOBAL_TOPO_CACHE: OnceLock<Mutex<Option<CachedGlobalContours>>> = OnceLock::new();
static GLOBAL_BATHYMETRY_CACHE: OnceLock<Mutex<Option<CachedGlobalContours>>> = OnceLock::new();

const CONTOUR_READ_RETRY_DELAY: Duration = Duration::from_millis(250);

/// Instantly drop every in-memory tile cache.
///
/// Forces a full reload on the next frame — both the global globe view and the
/// local terrain view.  Does NOT delete anything from disk; the SQLite cache
/// files are untouched and tiles will be re-read (not re-built) on demand.
pub fn blast_tile_caches() {
    if let Some(c) = LOCAL_CONTOUR_CACHE.get() {
        if let Ok(mut g) = c.lock() {
            g.reset_all();
        }
    }
    if let Some(c) = LUNAR_LOCAL_CONTOUR_CACHE.get() {
        if let Ok(mut g) = c.lock() {
            g.reset_all();
        }
    }
    if let Some(c) = MARS_LOCAL_CONTOUR_CACHE.get() {
        if let Ok(mut g) = c.lock() {
            g.reset_all();
        }
    }
    if let Some(c) = GLOBE_CONTOUR_CACHE.get() {
        if let Ok(mut g) = c.lock() {
            g.reset_all();
        }
    }
    if let Some(c) = LUNAR_GLOBE_CONTOUR_CACHE.get() {
        if let Ok(mut g) = c.lock() {
            g.reset_all();
        }
    }
    if let Some(c) = MARS_GLOBE_CONTOUR_CACHE.get() {
        if let Ok(mut g) = c.lock() {
            g.reset_all();
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
    /// Tiles already queued for a background DB read.
    in_flight: HashSet<(i32, i32)>,
    /// Insertion order for deterministic eviction among equal-distance ties
    order: Vec<(i32, i32)>,
    /// Contours from the previous zoom bucket, shown while new-resolution
    /// tiles are loading so the globe doesn't flash blank on zoom-level change.
    zoom_fallback: Option<Arc<Vec<ContourPath>>>,
    /// Memoized flattened globe merge. Keeping its `Arc` stable while the tile
    /// set is unchanged is critical: the GPU instance cache keys on this Arc's
    /// identity and must not rebuild on every repaint.
    merged: Option<Arc<Vec<ContourPath>>>,
    /// Monotonic tile-set revision used to memoize `merged` exactly, without a
    /// collision-prone per-frame hash over every cached tile.
    tiles_revision: u64,
    merged_revision: Option<u64>,
    /// Generation of tile-reader requests. A scene/cache reset advances this
    /// so a late worker cannot repopulate a newer cache state.
    load_epoch: u64,
    /// At most one SQLite/WKB reader runs for this cache at a time. It remains
    /// set across a reset until the old reader exits, preventing overlapping
    /// stale and current read workers.
    load_in_flight: Option<u64>,
    /// Temporary retry deadline after a reader cannot open/prepare the cache.
    /// It avoids repeatedly creating short-lived workers against a locked DB.
    read_retry_at: Option<Instant>,
}

impl Default for GlobeRegionCache {
    fn default() -> Self {
        Self {
            zoom_bucket: -1,
            root: None,
            tiles: HashMap::new(),
            in_flight: HashSet::new(),
            order: Vec::new(),
            zoom_fallback: None,
            merged: None,
            tiles_revision: 0,
            merged_revision: None,
            load_epoch: 0,
            load_in_flight: None,
            read_retry_at: None,
        }
    }
}

impl GlobeRegionCache {
    fn mark_tiles_changed(&mut self) {
        self.tiles_revision = self.tiles_revision.wrapping_add(1);
        self.merged = None;
        self.merged_revision = None;
    }

    /// Drop tiles for a new scene while preserving any old reader's ownership
    /// marker. That worker will discard its result once it notices the epoch.
    fn clear_tiles_for_new_scene(&mut self) {
        self.tiles.clear();
        self.in_flight.clear();
        self.order.clear();
        self.mark_tiles_changed();
        self.load_epoch = self.load_epoch.wrapping_add(1);
        self.read_retry_at = None;
    }

    fn reset_all(&mut self) {
        self.zoom_bucket = -1;
        self.root = None;
        self.zoom_fallback = None;
        self.clear_tiles_for_new_scene();
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
    /// Contours from the previous zoom level, kept as a fallback placeholder
    /// while tiles at the new zoom level are building / loading.  Cleared as
    /// soon as the new zoom has at least one tile in `entries`.
    zoom_fallback: Option<Arc<Vec<ContourPath>>>,
    /// Memoized flattened merge of every tile in `entries`, plus the signature
    /// of the entry set it was built from.  Rebuilt only when the set of tiles
    /// (or their contents) changes, so the per-frame call returns a cheap `Arc`
    /// clone instead of deep-cloning ~24k contour paths every frame.
    merged: Option<Arc<Vec<ContourPath>>>,
    entries_revision: u64,
    /// `None` is the ordinary all-tile merge; `Some(bits)` is a lunar/Mars
    /// overlap-partitioned merge at that exact bucket-step value.
    merged_key: Option<(u64, Option<u32>)>,
    load_epoch: u64,
    load_in_flight: Option<u64>,
    read_retry_at: Option<Instant>,
}

impl Default for LocalRegionCache {
    fn default() -> Self {
        Self {
            scene_key: None,
            entries: HashMap::new(),
            in_flight: HashSet::new(),
            zoom_fallback: None,
            merged: None,
            entries_revision: 0,
            merged_key: None,
            load_epoch: 0,
            load_in_flight: None,
            read_retry_at: None,
        }
    }
}

impl LocalRegionCache {
    fn mark_entries_changed(&mut self) {
        self.entries_revision = self.entries_revision.wrapping_add(1);
        self.merged = None;
        self.merged_key = None;
    }

    fn clear_entries_for_new_scene(&mut self) {
        self.entries.clear();
        self.in_flight.clear();
        self.mark_entries_changed();
        self.load_epoch = self.load_epoch.wrapping_add(1);
        self.read_retry_at = None;
    }

    fn reset_all(&mut self) {
        self.scene_key = None;
        self.zoom_fallback = None;
        self.clear_entries_for_new_scene();
    }
}

/// Return the flattened merge of all tiles in `cache`, rebuilding it only when
/// the entry set changed since the last call.  Returns `None` when empty.
fn merged_local_contours(cache: &mut LocalRegionCache) -> Option<Arc<Vec<ContourPath>>> {
    let merge_key = (cache.entries_revision, None);
    if cache.merged_key == Some(merge_key) {
        return cache.merged.clone();
    }
    if cache.entries.is_empty() {
        cache.merged = None;
        cache.merged_key = Some(merge_key);
        return None;
    }
    let mut merged = Vec::new();
    for contours in cache.entries.values() {
        merged.extend(contours.iter().cloned());
    }
    let arc = Arc::new(merged);
    cache.merged = Some(Arc::clone(&arc));
    cache.merged_key = Some(merge_key);
    Some(arc)
}

/// Merge overlapping lunar/Mars tile sets without redrawing the same contour
/// from neighbouring tiles. Like the ordinary local merge, this retains a
/// stable `Arc` while the cache entries are unchanged.
fn merged_partitioned_local_contours(
    cache: &mut LocalRegionCache,
    bucket_step: f32,
) -> Option<Arc<Vec<ContourPath>>> {
    let merge_key = (cache.entries_revision, Some(bucket_step.to_bits()));
    if cache.entries.is_empty() {
        cache.merged = None;
        cache.merged_key = Some(merge_key);
        return cache.zoom_fallback.clone();
    }

    if cache.merged_key == Some(merge_key) {
        if let Some(merged) = &cache.merged {
            return if merged.is_empty() {
                cache.zoom_fallback.clone()
            } else {
                Some(Arc::clone(merged))
            };
        }
    }

    let mut merged = Vec::new();
    for (key, contours) in &cache.entries {
        let tile_lat = key.lat_bucket as f32 * bucket_step;
        let tile_lon = key.lon_bucket as f32 * bucket_step;
        let half_step = bucket_step * 0.5;
        for contour in contours.iter() {
            if contour.points.is_empty() {
                continue;
            }
            let mid = &contour.points[contour.points.len() / 2];
            if (mid.lat - tile_lat).abs() <= half_step && (mid.lon - tile_lon).abs() <= half_step {
                merged.push(contour.clone());
            }
        }
    }

    let merged = Arc::new(merged);
    cache.merged = Some(Arc::clone(&merged));
    cache.merged_key = Some(merge_key);
    if merged.is_empty() {
        cache.zoom_fallback.clone()
    } else {
        cache.zoom_fallback = None;
        Some(merged)
    }
}

#[derive(Clone, PartialEq, Eq)]
struct SceneKey {
    root: Option<PathBuf>,
    anchor_lat_bucket: i32,
    anchor_lon_bucket: i32,
    zoom_bucket: i32,
}

type ContourReadRequest = (CacheKey, srtm_focus_cache::FocusContourAsset);

/// Claim one coalesced SQLite/WKB read for a local cache. New camera frames
/// leave their tiles unclaimed while a worker runs; its repaint then lets the
/// latest viewport submit the next batch.
fn begin_local_read(
    cache: &mut LocalRegionCache,
    assets: &[srtm_focus_cache::FocusContourAsset],
) -> Option<(u64, Vec<ContourReadRequest>)> {
    if cache.load_in_flight.is_some() {
        return None;
    }
    if cache
        .read_retry_at
        .is_some_and(|retry_at| retry_at > Instant::now())
    {
        return None;
    }
    cache.read_retry_at = None;

    let requests: Vec<_> = assets
        .iter()
        .filter_map(|asset| {
            let key = CacheKey {
                path: asset.path.clone(),
                lat_bucket: asset.lat_bucket,
                lon_bucket: asset.lon_bucket,
                zoom_bucket: asset.zoom_bucket,
            };
            (!cache.entries.contains_key(&key) && !cache.in_flight.contains(&key))
                .then(|| (key, asset.clone()))
        })
        .collect();
    if requests.is_empty() {
        return None;
    }

    for (key, _) in &requests {
        cache.in_flight.insert(key.clone());
    }
    let epoch = cache.load_epoch;
    cache.load_in_flight = Some(epoch);
    Some((epoch, requests))
}

/// Claim one coalesced SQLite/WKB read for a globe cache.
fn begin_globe_read(
    cache: &mut GlobeRegionCache,
    assets: &[srtm_focus_cache::FocusContourAsset],
) -> Option<(u64, Vec<ContourReadRequest>)> {
    if cache.load_in_flight.is_some() {
        return None;
    }
    if cache
        .read_retry_at
        .is_some_and(|retry_at| retry_at > Instant::now())
    {
        return None;
    }
    cache.read_retry_at = None;

    let requests: Vec<_> = assets
        .iter()
        .filter_map(|asset| {
            let tile_key = (asset.lat_bucket, asset.lon_bucket);
            (!cache.tiles.contains_key(&tile_key) && !cache.in_flight.contains(&tile_key)).then(
                || {
                    (
                        CacheKey {
                            path: asset.path.clone(),
                            lat_bucket: asset.lat_bucket,
                            lon_bucket: asset.lon_bucket,
                            zoom_bucket: asset.zoom_bucket,
                        },
                        asset.clone(),
                    )
                },
            )
        })
        .collect();
    if requests.is_empty() {
        return None;
    }

    for (key, _) in &requests {
        cache.in_flight.insert((key.lat_bucket, key.lon_bucket));
    }
    let epoch = cache.load_epoch;
    cache.load_in_flight = Some(epoch);
    Some((epoch, requests))
}

fn finish_local_read(
    cache: &Mutex<LocalRegionCache>,
    epoch: u64,
    requests: &[ContourReadRequest],
    loaded: Option<Vec<(CacheKey, Vec<ContourPath>)>>,
) {
    let Ok(mut cache) = cache.lock() else {
        return;
    };
    if cache.load_in_flight != Some(epoch) {
        return;
    }

    if cache.load_epoch == epoch {
        let read_failed = loaded.is_none();
        let mut changed = false;
        if let Some(loaded) = loaded {
            for (key, contours) in loaded {
                if !cache.entries.contains_key(&key) {
                    cache.entries.insert(key, Arc::new(contours));
                    changed = true;
                }
            }
        }
        for (key, _) in requests {
            cache.in_flight.remove(key);
        }
        if changed {
            cache.mark_entries_changed();
        }
        cache.read_retry_at = read_failed.then(|| Instant::now() + CONTOUR_READ_RETRY_DELAY);
    }
    cache.load_in_flight = None;
}

fn finish_globe_read(
    cache: &Mutex<GlobeRegionCache>,
    epoch: u64,
    requests: &[ContourReadRequest],
    loaded: Option<Vec<(CacheKey, Vec<ContourPath>)>>,
) {
    let Ok(mut cache) = cache.lock() else {
        return;
    };
    if cache.load_in_flight != Some(epoch) {
        return;
    }

    if cache.load_epoch == epoch {
        let read_failed = loaded.is_none();
        let mut changed = false;
        if let Some(loaded) = loaded {
            for (key, contours) in loaded {
                let tile_key = (key.lat_bucket, key.lon_bucket);
                if !cache.tiles.contains_key(&tile_key) {
                    cache.tiles.insert(tile_key, Arc::new(contours));
                    cache.order.push(tile_key);
                    changed = true;
                }
            }
        }
        for (key, _) in requests {
            cache.in_flight.remove(&(key.lat_bucket, key.lon_bucket));
        }
        if changed {
            cache.mark_tiles_changed();
        }
        cache.read_retry_at = read_failed.then(|| Instant::now() + CONTOUR_READ_RETRY_DELAY);
    }
    cache.load_in_flight = None;
}

fn spawn_local_read(
    cache: &'static Mutex<LocalRegionCache>,
    epoch: u64,
    requests: Vec<ContourReadRequest>,
    feature_budget: usize,
    ctx: egui::Context,
    worker_name: &'static str,
) {
    let cleanup_requests = requests.clone();
    let cleanup_ctx = ctx.clone();
    if let Err(error) = std::thread::Builder::new()
        .name(worker_name.into())
        .spawn(move || {
            let loaded = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                query_local_contours_batch(&requests[0].0.path, &requests, feature_budget).ok()
            })) {
                Ok(loaded) => loaded,
                Err(_) => {
                    eprintln!("[1kEE] {worker_name} panicked while reading contour tiles");
                    None
                }
            };
            let read_failed = loaded.is_none();
            finish_local_read(cache, epoch, &requests, loaded);
            if read_failed {
                ctx.request_repaint_after(CONTOUR_READ_RETRY_DELAY);
            } else {
                ctx.request_repaint();
            }
        })
    {
        finish_local_read(cache, epoch, &cleanup_requests, None);
        eprintln!("[1kEE] failed to spawn {worker_name}: {error}");
        cleanup_ctx.request_repaint_after(CONTOUR_READ_RETRY_DELAY);
    }
}

fn spawn_globe_read(
    cache: &'static Mutex<GlobeRegionCache>,
    epoch: u64,
    requests: Vec<ContourReadRequest>,
    feature_budget: usize,
    ctx: egui::Context,
    worker_name: &'static str,
) {
    let cleanup_requests = requests.clone();
    let cleanup_ctx = ctx.clone();
    if let Err(error) = std::thread::Builder::new()
        .name(worker_name.into())
        .spawn(move || {
            let loaded = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                query_local_contours_batch(&requests[0].0.path, &requests, feature_budget).ok()
            })) {
                Ok(loaded) => loaded,
                Err(_) => {
                    eprintln!("[1kEE] {worker_name} panicked while reading contour tiles");
                    None
                }
            };
            let read_failed = loaded.is_none();
            finish_globe_read(cache, epoch, &requests, loaded);
            if read_failed {
                ctx.request_repaint_after(CONTOUR_READ_RETRY_DELAY);
            } else {
                ctx.request_repaint();
            }
        })
    {
        finish_globe_read(cache, epoch, &cleanup_requests, None);
        eprintln!("[1kEE] failed to spawn {worker_name}: {error}");
        cleanup_ctx.request_repaint_after(CONTOUR_READ_RETRY_DELAY);
    }
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

    let cache: &'static Mutex<LocalRegionCache> =
        LOCAL_CONTOUR_CACHE.get_or_init(|| Mutex::new(LocalRegionCache::default()));
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

    // Phase 1: lock, reset stale scene state, and claim at most one batch
    // reader. While it runs, camera motion is coalesced into the next repaint.
    let read = {
        let mut guard = cache.lock().ok()?;
        if guard.scene_key.as_ref() != Some(&scene_key) {
            eprintln!(
                "[1kEE] scene change → {} assets for zoom_bucket={} (entries cleared)",
                assets.len(),
                assets.first().map(|a| a.zoom_bucket).unwrap_or(-1)
            );
            guard.scene_key = Some(scene_key);
            guard.clear_entries_for_new_scene();
        }
        begin_local_read(&mut guard, &assets)
    };

    if let Some((epoch, requests)) = read {
        spawn_local_read(
            cache,
            epoch,
            requests,
            per_asset_budget,
            ctx.clone(),
            "earth-local-contour-read",
        );
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
        guard.mark_entries_changed();
    }

    // Render ALL accumulated tiles, not just the current viewport grid.
    // Memoized: only rebuilds the flattened merge when the tile set changes.
    merged_local_contours(&mut guard)
}

/// Lunar analogue of `load_srtm_region_for_view` — sources from SLDEM2015 tiles
/// stored in `lunar_focus_cache.sqlite`.  Same streaming / caching pattern.
pub fn load_lunar_region_for_view(
    selected_root: Option<&Path>,
    scene_anchor: crate::model::GeoPoint,
    viewport_center: crate::model::GeoPoint,
    zoom: f32,
    _radius: i32,
    ctx: egui::Context,
) -> Option<Arc<Vec<ContourPath>>> {
    let assets =
        srtm_focus_cache::ensure_lunar_contour_region(selected_root, viewport_center, zoom);

    const MAX_LOCAL_TILES: usize = 200;

    let cache: &'static Mutex<LocalRegionCache> =
        LUNAR_LOCAL_CONTOUR_CACHE.get_or_init(|| Mutex::new(LocalRegionCache::default()));

    let current_zoom_bucket = srtm_focus_cache::zoom::lunar_spec_for_zoom(zoom).zoom_bucket;
    let per_asset_budget = (360usize / assets.len().max(1)).max(120);
    let bucket_step = srtm_focus_cache::lunar_half_extent_for_zoom(zoom) * 0.45;
    let scene_key = SceneKey {
        root: selected_root.map(Path::to_path_buf),
        anchor_lat_bucket: (scene_anchor.lat * 20.0).round() as i32,
        anchor_lon_bucket: (scene_anchor.lon * 20.0).round() as i32,
        zoom_bucket: current_zoom_bucket,
    };

    let read = {
        let mut guard = cache.lock().ok()?;
        if guard.scene_key.as_ref() != Some(&scene_key) {
            // Scene changed (usually a zoom level change).  Build a fallback
            // snapshot from the entries we have now so the user keeps seeing
            // the old resolution while the new tiles are building.
            let old_merged: Vec<ContourPath> = guard
                .entries
                .values()
                .flat_map(|v| v.iter().cloned())
                .collect();
            guard.zoom_fallback = if old_merged.is_empty() {
                None
            } else {
                Some(Arc::new(old_merged))
            };
            guard.scene_key = Some(scene_key);
            guard.clear_entries_for_new_scene();
        }
        if assets.is_empty() {
            return merged_partitioned_local_contours(&mut guard, bucket_step);
        }
        begin_local_read(&mut guard, &assets)
    };

    if let Some((epoch, requests)) = read {
        spawn_local_read(
            cache,
            epoch,
            requests,
            per_asset_budget,
            ctx.clone(),
            "lunar-local-contour-read",
        );
    }

    let mut guard = cache.lock().ok()?;

    if guard.entries.len() > MAX_LOCAL_TILES {
        // Evict the tiles farthest from the current viewport.
        let clat = (viewport_center.lat / bucket_step).round() as i32;
        let clon = (viewport_center.lon / bucket_step).round() as i32;
        let mut keys: Vec<CacheKey> = guard.entries.keys().cloned().collect();
        keys.sort_unstable_by_key(|k| {
            let dlat = k.lat_bucket - clat;
            let dlon = k.lon_bucket - clon;
            -(dlat * dlat + dlon * dlon)
        });
        let excess = guard.entries.len() - MAX_LOCAL_TILES;
        for k in keys.into_iter().take(excess) {
            guard.entries.remove(&k);
        }
        guard.mark_entries_changed();
    }

    merged_partitioned_local_contours(&mut guard, bucket_step)
}

/// Mars analogue of `load_srtm_region_for_view` — sources from MRO CTX tiles
/// stored in `mars_ctx_cache.sqlite`.
pub fn load_mars_region_for_view(
    selected_root: Option<&Path>,
    scene_anchor: crate::model::GeoPoint,
    viewport_center: crate::model::GeoPoint,
    zoom: f32,
    _radius: i32,
    ctx: egui::Context,
) -> Option<Arc<Vec<ContourPath>>> {
    let assets = srtm_focus_cache::ensure_mars_contour_region(selected_root, viewport_center, zoom);

    const MAX_LOCAL_TILES: usize = 200;

    let cache: &'static Mutex<LocalRegionCache> =
        MARS_LOCAL_CONTOUR_CACHE.get_or_init(|| Mutex::new(LocalRegionCache::default()));

    let current_zoom_bucket = srtm_focus_cache::zoom::mars_spec_for_zoom(zoom).zoom_bucket;
    let per_asset_budget = (360usize / assets.len().max(1)).max(120);
    let bucket_step = srtm_focus_cache::mars_half_extent_for_zoom(zoom) * 0.45;
    let scene_key = SceneKey {
        root: selected_root.map(Path::to_path_buf),
        anchor_lat_bucket: (scene_anchor.lat * 20.0).round() as i32,
        anchor_lon_bucket: (scene_anchor.lon * 20.0).round() as i32,
        zoom_bucket: current_zoom_bucket,
    };

    let read = {
        let mut guard = cache.lock().ok()?;
        if guard.scene_key.as_ref() != Some(&scene_key) {
            let old_merged: Vec<ContourPath> = guard
                .entries
                .values()
                .flat_map(|v| v.iter().cloned())
                .collect();
            guard.zoom_fallback = if old_merged.is_empty() {
                None
            } else {
                Some(Arc::new(old_merged))
            };
            guard.scene_key = Some(scene_key);
            guard.clear_entries_for_new_scene();
        }
        if assets.is_empty() {
            return merged_partitioned_local_contours(&mut guard, bucket_step);
        }
        begin_local_read(&mut guard, &assets)
    };

    if let Some((epoch, requests)) = read {
        spawn_local_read(
            cache,
            epoch,
            requests,
            per_asset_budget,
            ctx.clone(),
            "mars-local-contour-read",
        );
    }

    let mut guard = cache.lock().ok()?;

    if guard.entries.len() > MAX_LOCAL_TILES {
        let clat = (viewport_center.lat / bucket_step).round() as i32;
        let clon = (viewport_center.lon / bucket_step).round() as i32;
        let mut keys: Vec<CacheKey> = guard.entries.keys().cloned().collect();
        keys.sort_unstable_by_key(|k| {
            let dlat = k.lat_bucket - clat;
            let dlon = k.lon_bucket - clon;
            -(dlat * dlat + dlon * dlon)
        });
        let excess = guard.entries.len() - MAX_LOCAL_TILES;
        for k in keys.into_iter().take(excess) {
            guard.entries.remove(&k);
        }
        guard.mark_entries_changed();
    }

    merged_partitioned_local_contours(&mut guard, bucket_step)
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
    zoom: f32,
    ctx: egui::Context,
) -> Option<Arc<Vec<ContourPath>>> {
    const MAX_TILES: usize = 1600;
    // Map the actual globe view zoom to a coarse tile spec.  Globe mode caps
    // at bucket 1 (2.2°, 25 m) — finer tiles aren't visible on a globe and
    // cost far too much geometry.  radius=2 gives a 5×5 grid pre-fetched.
    let tile_zoom = globe_zoom_to_tile_zoom(zoom);

    let assets = srtm_focus_cache::ensure_focus_contour_region(selected_root, center, tile_zoom, 2);

    let cache: &'static Mutex<GlobeRegionCache> =
        GLOBE_CONTOUR_CACHE.get_or_init(|| Mutex::new(GlobeRegionCache::default()));
    let mut guard = cache.lock().ok()?;

    let zoom_bucket = srtm_focus_cache::zoom_bucket_for_zoom(tile_zoom);
    let root = selected_root.map(Path::to_path_buf);

    // On zoom-bucket or root change: snapshot current tiles as fallback so the
    // globe doesn't flash blank while new-resolution tiles are loading.
    if guard.zoom_bucket != zoom_bucket || guard.root != root {
        let old: Vec<ContourPath> = guard
            .tiles
            .values()
            .flat_map(|v| v.iter().cloned())
            .collect();
        guard.zoom_fallback = if old.is_empty() {
            None
        } else {
            Some(Arc::new(old))
        };
        guard.zoom_bucket = zoom_bucket;
        guard.root = root;
        guard.clear_tiles_for_new_scene();
    }

    if assets.is_empty() {
        // No SRTM root found; return whatever we already have.
        return render_globe_tiles(&mut guard);
    }

    let feature_budget = srtm_focus_cache::feature_budget_for_zoom(tile_zoom);
    let per_asset_budget = (feature_budget / assets.len().max(1)).max(120);

    let read = begin_globe_read(&mut guard, &assets);
    drop(guard);

    if let Some((epoch, requests)) = read {
        spawn_globe_read(
            cache,
            epoch,
            requests,
            per_asset_budget,
            ctx.clone(),
            "earth-globe-contour-read",
        );
    }

    // Re-acquire lock to render and evict from whatever is currently cached.
    let mut guard = cache.lock().ok()?;

    // Evict tiles furthest from centre when over the cap.
    if guard.tiles.len() > MAX_TILES {
        let half_extent = srtm_focus_cache::half_extent_for_zoom(tile_zoom);
        let bucket_step = half_extent * 0.45;
        let clat = (center.lat / bucket_step).round() as i32;
        let clon = (center.lon / bucket_step).round() as i32;

        // Sort order vec by distance ascending; keep the closest MAX_TILES.
        guard
            .order
            .sort_by_key(|&(lat, lon)| (lat - clat).pow(2) + (lon - clon).pow(2));
        let keep: std::collections::HashSet<(i32, i32)> =
            guard.order[..MAX_TILES].iter().copied().collect();
        let previous_len = guard.tiles.len();
        guard.tiles.retain(|k, _| keep.contains(k));
        guard.in_flight.retain(|k| keep.contains(k));
        guard.order.retain(|k| keep.contains(k));
        if guard.tiles.len() != previous_len {
            guard.mark_tiles_changed();
        }
    }

    render_globe_tiles(&mut guard)
}

/// Map globe view zoom to tile spec zoom for the contour cache.
///
/// Globe mode uses two tiers only:
/// - globe zoom < 2.5 → bucket 0 (3.6° tiles, 50 m interval) for wide/full-globe views
/// - globe zoom ≥ 2.5 → bucket 1 (2.2° tiles, 25 m interval) for hemisphere/continent
///
/// Capped at bucket 1 — finer tiles offer no visible benefit on a globe and
/// dramatically increase per-frame geometry.
fn globe_zoom_to_tile_zoom(globe_zoom: f32) -> f32 {
    if globe_zoom < 2.5 { 0.5 } else { 1.5 }
}

fn render_globe_tiles(guard: &mut GlobeRegionCache) -> Option<Arc<Vec<ContourPath>>> {
    if guard.tiles.is_empty() {
        // No new-resolution tiles yet — return the previous zoom level's
        // contours so the globe doesn't flash blank during the transition.
        return guard.zoom_fallback.clone();
    }

    if guard.merged_revision == Some(guard.tiles_revision) {
        if let Some(merged) = &guard.merged {
            return if merged.is_empty() {
                guard.zoom_fallback.clone()
            } else {
                Some(Arc::clone(merged))
            };
        }
    }

    // Sort by absolute elevation so output order is deterministic regardless
    // of HashMap iteration order. Without this, any budget-based subsetting in
    // the draw path would pick different contours each frame as tiles load or
    // unload, causing visible jitter.
    let mut merged: Vec<ContourPath> = guard
        .tiles
        .values()
        .flat_map(|v| v.iter().cloned())
        .collect();
    if merged.is_empty() {
        // A ready tile can legitimately contain no contours. Preserve the
        // previous zoom's geometry in that case just as the old un-memoized
        // path did, rather than flashing an empty globe.
        guard.merged = Some(Arc::new(merged));
        guard.merged_revision = Some(guard.tiles_revision);
        return guard.zoom_fallback.clone();
    }
    merged.sort_unstable_by(|a, b| {
        a.elevation_m
            .abs()
            .partial_cmp(&b.elevation_m.abs())
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // Real tiles are ready; drop the fallback to free memory and retain this
    // Arc until the tile set actually changes.
    guard.zoom_fallback = None;
    let merged = Arc::new(merged);
    guard.merged = Some(Arc::clone(&merged));
    guard.merged_revision = Some(guard.tiles_revision);
    Some(merged)
}

/// Lunar equivalent of `load_srtm_for_globe`.  Triggers on-demand tile builds
/// from the SLDEM2015 JP2, accumulates results in a separate cache, and returns
/// a merged arc over all ready tiles.
pub fn load_lunar_for_globe(
    selected_root: Option<&Path>,
    center: crate::model::GeoPoint,
    zoom: f32,
    ctx: egui::Context,
) -> Option<Arc<Vec<ContourPath>>> {
    const MAX_TILES: usize = 800;
    let tile_zoom = globe_zoom_to_tile_zoom(zoom);

    let assets = srtm_focus_cache::ensure_lunar_contour_region(selected_root, center, tile_zoom);

    let cache: &'static Mutex<GlobeRegionCache> =
        LUNAR_GLOBE_CONTOUR_CACHE.get_or_init(|| Mutex::new(GlobeRegionCache::default()));
    let mut guard = cache.lock().ok()?;

    let zoom_bucket = srtm_focus_cache::zoom_bucket_for_zoom(tile_zoom);
    let root = selected_root.map(Path::to_path_buf);

    if guard.zoom_bucket != zoom_bucket || guard.root != root {
        let old: Vec<ContourPath> = guard
            .tiles
            .values()
            .flat_map(|v| v.iter().cloned())
            .collect();
        guard.zoom_fallback = if old.is_empty() {
            None
        } else {
            Some(Arc::new(old))
        };
        guard.zoom_bucket = zoom_bucket;
        guard.root = root;
        guard.clear_tiles_for_new_scene();
    }

    if assets.is_empty() {
        return render_globe_tiles(&mut guard);
    }

    let per_asset_budget = (360 / assets.len().max(1)).max(120);

    let read = begin_globe_read(&mut guard, &assets);
    drop(guard);

    if let Some((epoch, requests)) = read {
        spawn_globe_read(
            cache,
            epoch,
            requests,
            per_asset_budget,
            ctx.clone(),
            "lunar-globe-contour-read",
        );
    }

    let mut guard = cache.lock().ok()?;

    if guard.tiles.len() > MAX_TILES {
        let half_extent = srtm_focus_cache::half_extent_for_zoom(tile_zoom);
        let bucket_step = half_extent * 0.45;
        let clat = (center.lat / bucket_step).round() as i32;
        let clon = (center.lon / bucket_step).round() as i32;
        guard
            .order
            .sort_by_key(|&(lat, lon)| (lat - clat).pow(2) + (lon - clon).pow(2));
        let keep: HashSet<(i32, i32)> = guard.order[..MAX_TILES].iter().copied().collect();
        let previous_len = guard.tiles.len();
        guard.tiles.retain(|k, _| keep.contains(k));
        guard.in_flight.retain(|k| keep.contains(k));
        guard.order.retain(|k| keep.contains(k));
        if guard.tiles.len() != previous_len {
            guard.mark_tiles_changed();
        }
    }

    render_globe_tiles(&mut guard)
}

/// Mars equivalent of `load_lunar_for_globe`. Triggers on-demand tile builds
/// from the CTX VRT, accumulates results in a separate cache, and returns
/// a merged arc over all ready tiles.
pub fn load_mars_for_globe(
    selected_root: Option<&Path>,
    center: crate::model::GeoPoint,
    zoom: f32,
    ctx: egui::Context,
) -> Option<Arc<Vec<ContourPath>>> {
    const MAX_TILES: usize = 800;
    let tile_zoom = globe_zoom_to_tile_zoom(zoom);

    let assets = srtm_focus_cache::ensure_mars_contour_region(selected_root, center, tile_zoom);

    let cache: &'static Mutex<GlobeRegionCache> =
        MARS_GLOBE_CONTOUR_CACHE.get_or_init(|| Mutex::new(GlobeRegionCache::default()));
    let mut guard = cache.lock().ok()?;

    let zoom_bucket = srtm_focus_cache::zoom_bucket_for_zoom(tile_zoom);
    let root = selected_root.map(Path::to_path_buf);

    if guard.zoom_bucket != zoom_bucket || guard.root != root {
        let old: Vec<ContourPath> = guard
            .tiles
            .values()
            .flat_map(|v| v.iter().cloned())
            .collect();
        guard.zoom_fallback = if old.is_empty() {
            None
        } else {
            Some(Arc::new(old))
        };
        guard.zoom_bucket = zoom_bucket;
        guard.root = root;
        guard.clear_tiles_for_new_scene();
    }

    if assets.is_empty() {
        return render_globe_tiles(&mut guard);
    }

    let per_asset_budget = (360 / assets.len().max(1)).max(120);

    let read = begin_globe_read(&mut guard, &assets);
    drop(guard);

    if let Some((epoch, requests)) = read {
        spawn_globe_read(
            cache,
            epoch,
            requests,
            per_asset_budget,
            ctx.clone(),
            "mars-globe-contour-read",
        );
    }

    let mut guard = cache.lock().ok()?;

    if guard.tiles.len() > MAX_TILES {
        let half_extent = srtm_focus_cache::half_extent_for_zoom(tile_zoom);
        let bucket_step = half_extent * 0.45;
        let clat = (center.lat / bucket_step).round() as i32;
        let clon = (center.lon / bucket_step).round() as i32;
        guard
            .order
            .sort_by_key(|&(lat, lon)| (lat - clat).pow(2) + (lon - clon).pow(2));
        let keep: HashSet<(i32, i32)> = guard.order[..MAX_TILES].iter().copied().collect();
        let previous_len = guard.tiles.len();
        guard.tiles.retain(|k, _| keep.contains(k));
        guard.in_flight.retain(|k| keep.contains(k));
        guard.order.retain(|k| keep.contains(k));
        if guard.tiles.len() != previous_len {
            guard.mark_tiles_changed();
        }
    }

    render_globe_tiles(&mut guard)
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
    let connection = srtm_focus_cache::db::open_cache_db_read_only(path)?;

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
    let connection = srtm_focus_cache::db::open_cache_db_read_only(path)?;
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
    let connection = srtm_focus_cache::db::open_cache_db_read_only(path)?;
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

fn query_local_contours_batch(
    path: &Path,
    requests: &[(CacheKey, srtm_focus_cache::FocusContourAsset)],
    feature_budget: usize,
) -> rusqlite::Result<Vec<(CacheKey, Vec<ContourPath>)>> {
    let connection = srtm_focus_cache::db::open_cache_db_read_only(path)?;
    let mut statement = connection.prepare(
        "SELECT geom, elevation_m
         FROM contour_tiles
         WHERE zoom_bucket = ?1 AND lat_bucket = ?2 AND lon_bucket = ?3
         ORDER BY ABS(elevation_m), fid",
    )?;

    let mut results = Vec::with_capacity(requests.len());
    let mut first_error = None;
    for (key, asset) in requests {
        let rows = match statement.query_map(
            params![key.zoom_bucket, key.lat_bucket, key.lon_bucket],
            |row| {
                let geometry: Vec<u8> = row.get(0)?;
                let elevation_m: f32 = row.get(1)?;
                Ok((geometry, elevation_m))
            },
        ) {
            Ok(rows) => rows,
            Err(error) => {
                eprintln!(
                    "[1kEE] contour tile z{} ({},{}) query failed: {error}",
                    key.zoom_bucket, key.lat_bucket, key.lon_bucket
                );
                if first_error.is_none() {
                    first_error = Some(error);
                }
                continue;
            }
        };

        let mut contours = Vec::new();
        let mut tile_failed = false;
        for row in rows {
            let (geometry, elevation_m) = match row {
                Ok(row) => row,
                Err(error) => {
                    eprintln!(
                        "[1kEE] contour tile z{} ({},{}) row failed: {error}",
                        key.zoom_bucket, key.lat_bucket, key.lon_bucket
                    );
                    if first_error.is_none() {
                        first_error = Some(error);
                    }
                    tile_failed = true;
                    break;
                }
            };
            for line in parse_gpkg_lines(&geometry) {
                if line.len() < 2 {
                    continue;
                }
                contours.push(ContourPath {
                    elevation_m,
                    points: simplify_line(line, asset.simplify_step),
                });
            }
        }
        if tile_failed {
            continue;
        }

        if contours.len() > feature_budget {
            let keep_step = contours.len().div_ceil(feature_budget.max(1));
            contours = contours
                .into_iter()
                .enumerate()
                .filter_map(|(index, contour)| (index % keep_step == 0).then_some(contour))
                .collect();
        }

        // Cache empty ready tiles too. Otherwise the render loop mistakes
        // nodata/flat tiles for misses and schedules the same SQLite/WKB read
        // again after every completed batch.
        results.push((key.clone(), contours));
    }

    // Preserve any successfully decoded tiles in a mixed batch. If every
    // request failed, surface the error so the single-flight scheduler applies
    // a short retry backoff instead of spinning one reader per repaint.
    if results.is_empty() {
        if let Some(error) = first_error {
            return Err(error);
        }
    }
    Ok(results)
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
            // Every direct child has at least one byte of endian marker, four
            // bytes of geometry type, and four bytes of point count. Validate
            // that minimum before reserving so a corrupt count cannot trigger
            // an enormous allocation.
            if count > wkb.len().checked_sub(cursor)? / 9 {
                return None;
            }
            let mut lines = Vec::with_capacity(count);
            for _ in 0..count {
                lines.push(parse_wkb_linestring(wkb, &mut cursor)?);
            }
            Some(lines)
        }
        _ => None,
    }
}

/// Parse a child of a WKB `MultiLineString`. WKB requires every child to be a
/// direct `LineString`, so deliberately rejecting nested collections keeps an
/// untrusted/corrupt cache blob from recursing an unnamed loader thread into a
/// stack overflow.
fn parse_wkb_linestring(wkb: &[u8], cursor: &mut usize) -> Option<Vec<GeoPoint>> {
    let endian = *wkb.get(*cursor)?;
    *cursor += 1;
    let little = endian == 1;
    let geom_type = read_u32(wkb, cursor, little)?;
    if geom_type % 1000 != 2 {
        return None;
    }
    parse_linestring(wkb, cursor, little)
}

fn parse_linestring(wkb: &[u8], cursor: &mut usize, little: bool) -> Option<Vec<GeoPoint>> {
    let count = read_u32(wkb, cursor, little)? as usize;
    // Each XY point occupies two f64 values. Check before reserving to reject
    // malformed count fields without a large allocation attempt.
    if count > wkb.len().checked_sub(*cursor)? / 16 {
        return None;
    }
    let mut points = Vec::with_capacity(count);
    for _ in 0..count {
        let lon = read_f64(wkb, cursor, little)? as f32;
        let lat = read_f64(wkb, cursor, little)? as f32;
        points.push(GeoPoint { lat, lon });
    }
    Some(points)
}

fn read_u32(bytes: &[u8], cursor: &mut usize, little: bool) -> Option<u32> {
    let end = (*cursor).checked_add(4)?;
    let slice = bytes.get(*cursor..end)?;
    *cursor = end;
    Some(if little {
        u32::from_le_bytes(slice.try_into().ok()?)
    } else {
        u32::from_be_bytes(slice.try_into().ok()?)
    })
}

fn read_f64(bytes: &[u8], cursor: &mut usize, little: bool) -> Option<f64> {
    let end = (*cursor).checked_add(8)?;
    let slice = bytes.get(*cursor..end)?;
    *cursor = end;
    Some(if little {
        f64::from_le_bytes(slice.try_into().ok()?)
    } else {
        f64::from_be_bytes(slice.try_into().ok()?)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::OptionalExtension;

    fn test_contour(elevation_m: f32) -> ContourPath {
        ContourPath {
            elevation_m,
            points: vec![
                GeoPoint { lat: 0.0, lon: 0.0 },
                GeoPoint { lat: 0.1, lon: 0.1 },
            ],
        }
    }

    fn test_asset(lat_bucket: i32, lon_bucket: i32) -> srtm_focus_cache::FocusContourAsset {
        srtm_focus_cache::FocusContourAsset {
            path: PathBuf::from("test-cache.sqlite"),
            simplify_step: 1,
            zoom_bucket: 0,
            lat_bucket,
            lon_bucket,
        }
    }

    fn test_gpkg_linestring() -> Vec<u8> {
        let mut geometry = b"GP\0\0\0\0\0\0".to_vec();
        geometry.push(1);
        geometry.extend_from_slice(&2u32.to_le_bytes());
        geometry.extend_from_slice(&2u32.to_le_bytes());
        for (lon, lat) in [(0.0f64, 0.0f64), (1.0, 1.0)] {
            geometry.extend_from_slice(&lon.to_le_bytes());
            geometry.extend_from_slice(&lat.to_le_bytes());
        }
        geometry
    }

    #[test]
    fn batch_reader_keeps_good_tiles_when_one_row_is_invalid() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after Unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "1kee-contour-batch-{}-{nonce}.sqlite",
            std::process::id()
        ));
        let connection = Connection::open(&path).expect("temporary contour cache");
        connection
            .execute_batch(
                "CREATE TABLE contour_tiles (
                    fid INTEGER PRIMARY KEY,
                    zoom_bucket INTEGER,
                    lat_bucket INTEGER,
                    lon_bucket INTEGER,
                    geom BLOB,
                    elevation_m
                )",
            )
            .expect("contour table");
        connection
            .execute(
                "INSERT INTO contour_tiles
                    (fid, zoom_bucket, lat_bucket, lon_bucket, geom, elevation_m)
                 VALUES (1, 0, 0, 0, ?1, 10.0)",
                rusqlite::params![test_gpkg_linestring()],
            )
            .expect("valid contour row");
        connection
            .execute(
                "INSERT INTO contour_tiles
                    (fid, zoom_bucket, lat_bucket, lon_bucket, geom, elevation_m)
                 VALUES (2, 0, 1, 0, ?1, 'not-a-number')",
                rusqlite::params![test_gpkg_linestring()],
            )
            .expect("invalid contour row");
        drop(connection);

        let asset = srtm_focus_cache::FocusContourAsset {
            path: path.clone(),
            simplify_step: 1,
            zoom_bucket: 0,
            lat_bucket: 0,
            lon_bucket: 0,
        };
        let requests = vec![
            (
                CacheKey {
                    path: path.clone(),
                    lat_bucket: 0,
                    lon_bucket: 0,
                    zoom_bucket: 0,
                },
                asset.clone(),
            ),
            (
                CacheKey {
                    path: path.clone(),
                    lat_bucket: 1,
                    lon_bucket: 0,
                    zoom_bucket: 0,
                },
                srtm_focus_cache::FocusContourAsset {
                    lat_bucket: 1,
                    ..asset
                },
            ),
        ];

        let loaded = query_local_contours_batch(&path, &requests, 120)
            .expect("valid tile should survive neighbouring row failure");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].0.lat_bucket, 0);
        assert!(!loaded[0].1.is_empty());
        std::fs::remove_file(path).expect("remove temporary contour cache");
    }

    #[test]
    fn stale_globe_read_cannot_repopulate_a_reset_cache() {
        let asset = test_asset(4, -7);
        let mut state = GlobeRegionCache::default();
        let (epoch, requests) =
            begin_globe_read(&mut state, std::slice::from_ref(&asset)).expect("first read");
        assert!(begin_globe_read(&mut state, std::slice::from_ref(&asset)).is_none());

        state.reset_all();
        assert_ne!(state.load_epoch, epoch);
        let cache = Mutex::new(state);
        finish_globe_read(
            &cache,
            epoch,
            &requests,
            Some(vec![(requests[0].0.clone(), vec![test_contour(10.0)])]),
        );

        let mut state = cache.lock().expect("test cache lock");
        assert!(state.tiles.is_empty());
        assert_eq!(state.load_in_flight, None);
        assert!(begin_globe_read(&mut state, std::slice::from_ref(&asset)).is_some());
    }

    #[test]
    fn globe_merge_reuses_its_arc_until_the_tile_set_changes() {
        let mut cache = GlobeRegionCache::default();
        cache
            .tiles
            .insert((0, 0), Arc::new(vec![test_contour(100.0)]));
        cache.mark_tiles_changed();

        let first = render_globe_tiles(&mut cache).expect("first merged globe tile");
        let second = render_globe_tiles(&mut cache).expect("cached globe merge");
        assert!(Arc::ptr_eq(&first, &second));

        cache
            .tiles
            .insert((0, 1), Arc::new(vec![test_contour(200.0)]));
        cache.mark_tiles_changed();
        let updated = render_globe_tiles(&mut cache).expect("updated globe merge");
        assert!(!Arc::ptr_eq(&first, &updated));
        assert_eq!(updated.len(), 2);

        cache.tiles.remove(&(0, 1));
        cache.mark_tiles_changed();
        let evicted = render_globe_tiles(&mut cache).expect("evicted globe merge");
        assert!(!Arc::ptr_eq(&updated, &evicted));
        assert_eq!(evicted.len(), 1);
    }

    #[test]
    fn globe_merge_keeps_the_zoom_fallback_for_empty_tiles() {
        let fallback = Arc::new(vec![test_contour(100.0)]);
        let mut cache = GlobeRegionCache {
            zoom_fallback: Some(Arc::clone(&fallback)),
            ..Default::default()
        };
        cache.tiles.insert((0, 0), Arc::new(Vec::new()));
        cache.mark_tiles_changed();

        let first = render_globe_tiles(&mut cache).expect("zoom fallback");
        let second = render_globe_tiles(&mut cache).expect("cached zoom fallback");
        assert!(Arc::ptr_eq(&first, &fallback));
        assert!(Arc::ptr_eq(&second, &fallback));
    }

    #[test]
    fn wkb_parser_rejects_nested_multiline_collections_without_recursing() {
        let mut wkb = Vec::new();
        wkb.push(1);
        wkb.extend_from_slice(&2u32.to_le_bytes());
        wkb.extend_from_slice(&2u32.to_le_bytes());
        for (lon, lat) in [(0.0f64, 0.0f64), (1.0, 1.0)] {
            wkb.extend_from_slice(&lon.to_le_bytes());
            wkb.extend_from_slice(&lat.to_le_bytes());
        }

        // MultiLineString children must be LineStrings. Build deeply nested
        // invalid collections iteratively so the test itself uses no recursion.
        for _ in 0..64 {
            let mut parent = Vec::with_capacity(wkb.len() + 9);
            parent.push(1);
            parent.extend_from_slice(&5u32.to_le_bytes());
            parent.extend_from_slice(&1u32.to_le_bytes());
            parent.extend_from_slice(&wkb);
            wkb = parent;
        }

        assert!(parse_wkb_geometry(&wkb).is_none());
    }

    #[test]
    fn wkb_parser_accepts_direct_multiline_linestring_children() {
        let mut wkb = Vec::new();
        wkb.push(1);
        wkb.extend_from_slice(&5u32.to_le_bytes());
        wkb.extend_from_slice(&1u32.to_le_bytes());
        wkb.push(1);
        wkb.extend_from_slice(&2u32.to_le_bytes());
        wkb.extend_from_slice(&2u32.to_le_bytes());
        for (lon, lat) in [(12.5f64, -3.0f64), (13.0, -2.5)] {
            wkb.extend_from_slice(&lon.to_le_bytes());
            wkb.extend_from_slice(&lat.to_le_bytes());
        }

        let lines = parse_wkb_geometry(&wkb).expect("valid multiline geometry");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].len(), 2);
        assert_eq!(lines[0][0].lon, 12.5);
        assert_eq!(lines[0][0].lat, -3.0);
    }

    #[test]
    fn wkb_parser_rejects_impossible_point_count_before_allocating() {
        let mut wkb = Vec::new();
        wkb.push(1);
        wkb.extend_from_slice(&2u32.to_le_bytes());
        wkb.extend_from_slice(&u32::MAX.to_le_bytes());
        assert!(parse_wkb_geometry(&wkb).is_none());
    }

    #[test]
    fn reads_cached_sqlite_focus_contours() {
        let path = Path::new("Derived/terrain/srtm_focus_cache.sqlite");
        if !path.exists() {
            return;
        }

        let connection = srtm_focus_cache::db::open_cache_db_read_only(path)
            .expect("should open shared SRTM cache DB");
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

        let request = (
            CacheKey {
                path: path.to_path_buf(),
                zoom_bucket,
                lat_bucket,
                lon_bucket,
            },
            srtm_focus_cache::FocusContourAsset {
                path: path.to_path_buf(),
                simplify_step: 2,
                zoom_bucket,
                lat_bucket,
                lon_bucket,
            },
        );
        let contours = query_local_contours_batch(path, &[request], 1_500)
            .expect("should read cached SRTM focus contours")
            .into_iter()
            .next()
            .map(|(_, contours)| contours)
            .unwrap_or_default();
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
