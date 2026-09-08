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
/// Keep local-view reads responsive even when the prefetch envelope includes
/// many cached tiles. The next repaint claims the following nearest batch.
const LOCAL_CONTOUR_READ_BATCH_SIZE: usize = 8;
/// Keep a wide decoded-tile envelope while the viewport moves, without
/// retaining an unbounded trail across a long local-terrain session.
const LOCAL_CONTOUR_RETAIN_RADIUS: i32 = 12;
/// Refresh a stationary local manifest periodically so companion cache-builder
/// writes become visible without polling SQLite every paint.
const LOCAL_MANIFEST_SNAPSHOT_TTL: Duration = Duration::from_millis(350);

/// Instantly drop every in-memory tile cache.
///
/// Forces a full reload on the next frame — both the global globe view and the
/// local terrain view.  Does NOT delete anything from disk; the SQLite cache
/// files are untouched and tiles will be re-read (not re-built) on demand.
pub fn blast_tile_caches() {
    srtm_focus_cache::clear_contour_build_backoffs();
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
    super::local_terrain_scene::hillshade_layer::clear();
}

/// Whether the most recently selected local manifest window still needs tiles.
/// The world-map repaint scheduler uses this in front of the paint pass, so it
/// must remain an in-memory check rather than opening the cache database again.
pub fn local_contours_pending(active_body: crate::model::ActiveBody) -> bool {
    let cache = match active_body {
        crate::model::ActiveBody::Earth => LOCAL_CONTOUR_CACHE.get(),
        crate::model::ActiveBody::Moon => LUNAR_LOCAL_CONTOUR_CACHE.get(),
        crate::model::ActiveBody::Mars => MARS_LOCAL_CONTOUR_CACHE.get(),
    };
    cache
        .and_then(|cache| cache.lock().ok())
        .map(|cache| {
            cache.load_in_flight.is_some()
                || cache
                    .last_status
                    .map(|status| status.ready_assets < status.total_assets)
                    .unwrap_or(true)
        })
        .unwrap_or(true)
}

#[derive(Clone)]
pub struct ContourPath {
    pub elevation_m: f32,
    pub points: Vec<GeoPoint>,
}

/// One local-terrain frame's decoded contours plus its ready/progress snapshot.
/// The scene uses this for its pulse grid and progress card instead of opening
/// SQLite again; a compatible prior manifest supplies known overlap while the
/// exact current manifest refreshes in the background.
#[derive(Clone)]
pub struct LocalContourLoad {
    pub contours: Option<Arc<Vec<ContourPath>>>,
    pub ready_buckets: HashSet<(i32, i32)>,
    pub status: srtm_focus_cache::FocusContourRegionStatus,
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

#[derive(Clone, PartialEq, Eq)]
struct LocalManifestKey {
    root: Option<PathBuf>,
    center_lat_bucket: i32,
    center_lon_bucket: i32,
    zoom_bucket: i32,
    prefetch_radius: i32,
    build_radius: i32,
}

#[derive(Clone)]
struct LocalManifestSnapshot {
    key: LocalManifestKey,
    assets: Vec<srtm_focus_cache::FocusContourAsset>,
    manifest_revision: u64,
    refreshed_at: Instant,
}

/// The two views a local paint frame needs while a manifest refresh is in
/// flight. Only an exact snapshot can schedule new SQLite/WKB reads; a
/// compatible prior snapshot may still describe overlapping ready tiles for
/// the loading overlay during an overlapping pan.
#[derive(Default)]
struct LocalManifestAssetViews {
    assets: Vec<srtm_focus_cache::FocusContourAsset>,
    exact_for_reads: bool,
}

impl LocalManifestAssetViews {
    fn reader_assets(&self) -> &[srtm_focus_cache::FocusContourAsset] {
        if self.exact_for_reads {
            &self.assets
        } else {
            &[]
        }
    }

    fn display_assets(&self) -> &[srtm_focus_cache::FocusContourAsset] {
        &self.assets
    }
}

/// Identity for an asynchronously flattened local contour window.  The tile
/// cache stores decoded geometry per tile; this key makes a worker result
/// publishable only when it still matches the latest camera window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LocalMergeKey {
    load_epoch: u64,
    entries_revision: u64,
    partition_bucket_step_bits: Option<u32>,
    center_lat_bucket: i32,
    center_lon_bucket: i32,
    source_radius: i32,
}

#[derive(Clone, Copy)]
struct LocalMergeSpec {
    center_lat_bucket: i32,
    center_lon_bucket: i32,
    source_radius: i32,
    partition_bucket_step: Option<f32>,
}

struct LocalMergeWork {
    key: LocalMergeKey,
    tiles: Vec<(CacheKey, Arc<Vec<ContourPath>>)>,
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
    manifest_snapshot: Option<LocalManifestSnapshot>,
    /// Most recent manifest request, which may be newer than the published
    /// snapshot while a slow SQLite/root lookup is still running.
    manifest_requested_key: Option<LocalManifestKey>,
    /// At most one manifest/build-selection worker per terrain body.  Camera
    /// motion updates `manifest_requested_key`; the next repaint coalesces to
    /// it after this worker completes.
    manifest_in_flight: Option<LocalManifestKey>,
    last_status: Option<srtm_focus_cache::FocusContourRegionStatus>,
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
    /// Most recently completed flattened merge.  It intentionally remains
    /// drawable while a replacement is built on a worker, so a newly decoded
    /// batch cannot make the paint thread clone the full source envelope.
    merged: Option<Arc<Vec<ContourPath>>>,
    entries_revision: u64,
    /// Key of the completed `merged` value.
    merged_key: Option<LocalMergeKey>,
    /// The latest window requested by paint.  A worker whose camera window is
    /// no longer current discards its result instead of briefly drawing stale
    /// geometry after a pan.
    merge_requested_key: Option<LocalMergeKey>,
    /// Single-flight background flatten/partition worker marker.
    merge_in_flight: Option<LocalMergeKey>,
    load_epoch: u64,
    load_in_flight: Option<u64>,
    read_retry_at: Option<Instant>,
}

impl Default for LocalRegionCache {
    fn default() -> Self {
        Self {
            scene_key: None,
            manifest_snapshot: None,
            manifest_requested_key: None,
            manifest_in_flight: None,
            last_status: None,
            entries: HashMap::new(),
            in_flight: HashSet::new(),
            zoom_fallback: None,
            merged: None,
            entries_revision: 0,
            merged_key: None,
            merge_requested_key: None,
            merge_in_flight: None,
            load_epoch: 0,
            load_in_flight: None,
            read_retry_at: None,
        }
    }
}

impl LocalRegionCache {
    fn mark_entries_changed(&mut self) {
        self.entries_revision = self.entries_revision.wrapping_add(1);
        // Keep the previous complete merge visible until a worker replaces it.
        // Dropping it here would turn every tile arrival into a blank frame.
        self.merged_key = None;
    }

    fn clear_entries_for_new_scene(&mut self) {
        self.entries.clear();
        self.in_flight.clear();
        self.last_status = None;
        self.merged = None;
        self.merged_key = None;
        self.merge_requested_key = None;
        self.merge_in_flight = None;
        self.mark_entries_changed();
        self.load_epoch = self.load_epoch.wrapping_add(1);
        self.read_retry_at = None;
    }

    fn reset_all(&mut self) {
        self.scene_key = None;
        self.manifest_snapshot = None;
        self.manifest_requested_key = None;
        self.manifest_in_flight = None;
        self.last_status = None;
        self.zoom_fallback = None;
        self.clear_entries_for_new_scene();
    }
}

fn local_tile_distance(key: &CacheKey, center_lat_bucket: i32, center_lon_bucket: i32) -> i32 {
    (key.lat_bucket - center_lat_bucket)
        .abs()
        .max((key.lon_bucket - center_lon_bucket).abs())
}

fn retain_local_entries(
    cache: &mut LocalRegionCache,
    center_lat_bucket: i32,
    center_lon_bucket: i32,
) {
    let before = cache.entries.len();
    cache.entries.retain(|key, _| {
        local_tile_distance(key, center_lat_bucket, center_lon_bucket)
            <= LOCAL_CONTOUR_RETAIN_RADIUS
    });

    if cache.entries.len() != before {
        cache.mark_entries_changed();
    }
}

/// Claim a background flatten/partition job.  The only UI-thread work here is
/// cloning tile `Arc`s under the short cache lock; copying individual contour
/// paths and points happens after the lock has been released.
fn begin_local_merge(cache: &mut LocalRegionCache, spec: LocalMergeSpec) -> Option<LocalMergeWork> {
    let key = LocalMergeKey {
        load_epoch: cache.load_epoch,
        entries_revision: cache.entries_revision,
        partition_bucket_step_bits: spec.partition_bucket_step.map(f32::to_bits),
        center_lat_bucket: spec.center_lat_bucket,
        center_lon_bucket: spec.center_lon_bucket,
        source_radius: spec.source_radius,
    };
    cache.merge_requested_key = Some(key);
    if cache.merged_key == Some(key) || cache.merge_in_flight.is_some() {
        return None;
    }

    let tiles = cache
        .entries
        .iter()
        .filter(|(tile, _)| {
            local_tile_distance(tile, spec.center_lat_bucket, spec.center_lon_bucket)
                <= spec.source_radius
        })
        .map(|(tile, contours)| (tile.clone(), Arc::clone(contours)))
        .collect::<Vec<_>>();
    if tiles.is_empty() {
        cache.merged = None;
        cache.merged_key = Some(key);
        return None;
    }

    cache.merge_in_flight = Some(key);
    Some(LocalMergeWork { key, tiles })
}

/// Flatten a snapshot of tile `Arc`s, keeping exclusive midpoint ownership for
/// lunar and Mars contour tiles. This runs only on named merge workers.
fn flatten_local_merge(work: LocalMergeWork) -> Arc<Vec<ContourPath>> {
    let capacity = work.tiles.iter().map(|(_, contours)| contours.len()).sum();
    let mut merged = Vec::with_capacity(capacity);
    let partition_bucket_step = work.key.partition_bucket_step_bits.map(f32::from_bits);

    for (tile, contours) in work.tiles {
        let Some(bucket_step) = partition_bucket_step else {
            merged.extend(contours.iter().cloned());
            continue;
        };

        let tile_lat = tile.lat_bucket as f32 * bucket_step;
        let tile_lon = tile.lon_bucket as f32 * bucket_step;
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

    Arc::new(merged)
}

fn finish_local_merge(
    cache: &Mutex<LocalRegionCache>,
    key: LocalMergeKey,
    merged: Option<Arc<Vec<ContourPath>>>,
) {
    let Ok(mut cache) = cache.lock() else {
        return;
    };
    if cache.merge_in_flight == Some(key) {
        cache.merge_in_flight = None;
    }
    if cache.load_epoch != key.load_epoch
        || cache.entries_revision != key.entries_revision
        || cache.merge_requested_key != Some(key)
    {
        return;
    }

    if let Some(merged) = merged {
        if key.partition_bucket_step_bits.is_some() && !merged.is_empty() {
            cache.zoom_fallback = None;
        }
        cache.merged = Some(merged);
        cache.merged_key = Some(key);
    }
}

fn spawn_local_merge(
    cache: &'static Mutex<LocalRegionCache>,
    work: LocalMergeWork,
    ctx: egui::Context,
    worker_name: &'static str,
) {
    let key = work.key;
    let cleanup_ctx = ctx.clone();
    if let Err(error) = std::thread::Builder::new()
        .name(worker_name.into())
        .spawn(move || {
            let merged = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                flatten_local_merge(work)
            })) {
                Ok(merged) => Some(merged),
                Err(_) => {
                    eprintln!("[1kEE] {worker_name} panicked while merging contour tiles");
                    None
                }
            };
            finish_local_merge(cache, key, merged);
            ctx.request_repaint();
        })
    {
        finish_local_merge(cache, key, None);
        eprintln!("[1kEE] failed to spawn {worker_name}: {error}");
        cleanup_ctx.request_repaint();
    }
}

#[cfg(test)]
fn merged_local_contours(
    cache: &LocalRegionCache,
    center_lat_bucket: i32,
    center_lon_bucket: i32,
    source_radius: i32,
) -> Option<Arc<Vec<ContourPath>>> {
    let tiles = cache
        .entries
        .iter()
        .filter(|(tile, _)| {
            local_tile_distance(tile, center_lat_bucket, center_lon_bucket) <= source_radius
        })
        .map(|(tile, contours)| (tile.clone(), Arc::clone(contours)))
        .collect();
    let merged = flatten_local_merge(LocalMergeWork {
        key: LocalMergeKey {
            load_epoch: cache.load_epoch,
            entries_revision: cache.entries_revision,
            partition_bucket_step_bits: None,
            center_lat_bucket,
            center_lon_bucket,
            source_radius,
        },
        tiles,
    });
    (!merged.is_empty()).then_some(merged)
}

#[cfg(test)]
fn merged_partitioned_local_contours(
    cache: &LocalRegionCache,
    bucket_step: f32,
    center_lat_bucket: i32,
    center_lon_bucket: i32,
    source_radius: i32,
) -> Option<Arc<Vec<ContourPath>>> {
    let tiles = cache
        .entries
        .iter()
        .filter(|(tile, _)| {
            local_tile_distance(tile, center_lat_bucket, center_lon_bucket) <= source_radius
        })
        .map(|(tile, contours)| (tile.clone(), Arc::clone(contours)))
        .collect();
    let merged = flatten_local_merge(LocalMergeWork {
        key: LocalMergeKey {
            load_epoch: cache.load_epoch,
            entries_revision: cache.entries_revision,
            partition_bucket_step_bits: Some(bucket_step.to_bits()),
            center_lat_bucket,
            center_lon_bucket,
            source_radius,
        },
        tiles,
    });
    (!merged.is_empty()).then_some(merged)
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
    center_lat_bucket: i32,
    center_lon_bucket: i32,
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

    let mut requests: Vec<_> = assets
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
    // A wide prefetch envelope must not make the first visible contours wait
    // for every outer tile. Read the nearest tiles first, then let the worker
    // repaint/coalesce the next bounded batch.
    requests.sort_unstable_by_key(|(key, _)| {
        local_tile_distance(key, center_lat_bucket, center_lon_bucket)
    });
    requests.truncate(LOCAL_CONTOUR_READ_BATCH_SIZE);
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

/// Publish a background manifest selection only when its camera key remains
/// current. This keeps a slow/locked SQLite volume from ever blocking paint.
fn finish_local_manifest(
    cache: &Mutex<LocalRegionCache>,
    key: &LocalManifestKey,
    assets: Option<Vec<srtm_focus_cache::FocusContourAsset>>,
) {
    let Ok(mut cache) = cache.lock() else {
        return;
    };
    if cache.manifest_in_flight.as_ref() == Some(key) {
        cache.manifest_in_flight = None;
    }
    if cache.manifest_requested_key.as_ref() != Some(key) {
        return;
    }
    if let Some(assets) = assets {
        cache.manifest_snapshot = Some(LocalManifestSnapshot {
            key: key.clone(),
            assets,
            // Observe the revision after selection: a builder that completed
            // during the query is already represented by this snapshot.
            manifest_revision: srtm_focus_cache::contour_manifest_revision(),
            refreshed_at: Instant::now(),
        });
    }
}

/// Return the asset views that are safe for the requested local window.
///
/// An exact snapshot can drive both rendering progress and new read requests.
/// During a compatible pan, the old snapshot remains useful only to identify
/// overlapping ready tiles: submitting it to the reader would make stale
/// manifest selection drive new I/O. Restricting that fallback to the new
/// prefetch window prevents a short manifest handoff from reporting every
/// local tile as missing while the retained contour merge remains visible.
fn local_manifest_asset_views(
    snapshot: Option<&LocalManifestSnapshot>,
    requested_key: &LocalManifestKey,
) -> LocalManifestAssetViews {
    let Some(snapshot) = snapshot else {
        return LocalManifestAssetViews::default();
    };

    if snapshot.key == *requested_key {
        return LocalManifestAssetViews {
            assets: snapshot.assets.clone(),
            exact_for_reads: true,
        };
    }

    let same_display_contract = snapshot.key.root == requested_key.root
        && snapshot.key.zoom_bucket == requested_key.zoom_bucket
        && snapshot.key.prefetch_radius == requested_key.prefetch_radius
        && snapshot.key.build_radius == requested_key.build_radius;
    if !same_display_contract {
        return LocalManifestAssetViews::default();
    }

    let display_assets = snapshot
        .assets
        .iter()
        .filter(|asset| {
            asset.zoom_bucket == requested_key.zoom_bucket
                && (i64::from(asset.lat_bucket) - i64::from(requested_key.center_lat_bucket))
                    .abs()
                    .max(
                        (i64::from(asset.lon_bucket) - i64::from(requested_key.center_lon_bucket))
                            .abs(),
                    )
                    <= i64::from(requested_key.prefetch_radius)
        })
        .cloned()
        .collect();
    LocalManifestAssetViews {
        assets: display_assets,
        exact_for_reads: false,
    }
}

/// Reuse a local manifest selection while the camera stays in the same bucket.
/// SQLite/root discovery and on-demand build selection run in a single worker.
/// While a compatible pan waits for that worker, paint uses the overlapping
/// prior snapshot only for progress/pulse display; read scheduling still waits
/// for an exact current manifest.
fn local_manifest_assets<F>(
    cache: &'static Mutex<LocalRegionCache>,
    key: LocalManifestKey,
    ctx: egui::Context,
    worker_name: &'static str,
    fetch: F,
) -> LocalManifestAssetViews
where
    F: FnOnce() -> Vec<srtm_focus_cache::FocusContourAsset> + Send + 'static,
{
    let manifest_revision = srtm_focus_cache::contour_manifest_revision();
    let (asset_views, start_worker) = if let Ok(mut guard) = cache.lock() {
        let asset_views = local_manifest_asset_views(guard.manifest_snapshot.as_ref(), &key);
        let snapshot_is_fresh = guard.manifest_snapshot.as_ref().is_some_and(|snapshot| {
            snapshot.key == key
                && snapshot.manifest_revision == manifest_revision
                && snapshot.refreshed_at.elapsed() < LOCAL_MANIFEST_SNAPSHOT_TTL
        });
        if snapshot_is_fresh {
            // A previous slow request may still be in flight for another
            // viewport. Record this key before returning so that worker
            // cannot overwrite the fresh snapshot after a quick pan back.
            guard.manifest_requested_key = Some(key);
            return asset_views;
        }
        guard.manifest_requested_key = Some(key.clone());
        let start_worker = guard.manifest_in_flight.is_none();
        if start_worker {
            guard.manifest_in_flight = Some(key.clone());
        }
        (asset_views, start_worker)
    } else {
        (LocalManifestAssetViews::default(), false)
    };

    if start_worker {
        let worker_key = key.clone();
        let cleanup_key = worker_key.clone();
        let cleanup_ctx = ctx.clone();
        if let Err(error) = std::thread::Builder::new()
            .name(worker_name.into())
            .spawn(move || {
                let assets = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(fetch)) {
                    Ok(assets) => Some(assets),
                    Err(_) => {
                        eprintln!("[1kEE] {worker_name} panicked while selecting contour tiles");
                        None
                    }
                };
                finish_local_manifest(cache, &worker_key, assets);
                ctx.request_repaint();
            })
        {
            finish_local_manifest(cache, &cleanup_key, None);
            eprintln!("[1kEE] failed to spawn {worker_name}: {error}");
            cleanup_ctx.request_repaint();
        }
    }

    asset_views
}

pub fn load_srtm_region_for_view(
    selected_root: Option<&Path>,
    scene_anchor: GeoPoint,
    viewport_center: GeoPoint,
    zoom: f32,
    prefetch_radius: i32,
    build_radius: i32,
    ctx: egui::Context,
) -> LocalContourLoad {
    let prefetch_radius = prefetch_radius.clamp(0, 16);
    let build_radius = build_radius.clamp(0, prefetch_radius);

    let cache: &'static Mutex<LocalRegionCache> =
        LOCAL_CONTOUR_CACHE.get_or_init(|| Mutex::new(LocalRegionCache::default()));
    let bucket_step = srtm_focus_cache::half_extent_for_zoom(zoom) * 0.45;
    let center_lat_bucket = (viewport_center.lat / bucket_step).round() as i32;
    let center_lon_bucket = (viewport_center.lon / bucket_step).round() as i32;
    let manifest_key = LocalManifestKey {
        root: selected_root.map(Path::to_path_buf),
        center_lat_bucket,
        center_lon_bucket,
        zoom_bucket: srtm_focus_cache::zoom_bucket_for_zoom(zoom),
        prefetch_radius,
        build_radius,
    };
    let manifest_root = selected_root.map(Path::to_path_buf);
    let assets = local_manifest_assets(
        cache,
        manifest_key,
        ctx.clone(),
        "earth-local-contour-manifest",
        move || {
            srtm_focus_cache::ensure_focus_contour_region(
                manifest_root.as_deref(),
                viewport_center,
                zoom,
                prefetch_radius,
                build_radius,
            )
        },
    );
    let state = srtm_focus_cache::local_contour_region_state(
        crate::model::ActiveBody::Earth,
        assets.display_assets(),
        viewport_center,
        zoom,
        build_radius,
    );
    let feature_budget = srtm_focus_cache::feature_budget_for_zoom(zoom);
    let per_asset_budget = (feature_budget / assets.reader_assets().len().max(1)).max(120);
    let scene_key = SceneKey {
        root: selected_root.map(Path::to_path_buf),
        anchor_lat_bucket: (scene_anchor.lat * 20.0).round() as i32,
        anchor_lon_bucket: (scene_anchor.lon * 20.0).round() as i32,
        zoom_bucket: srtm_focus_cache::zoom_bucket_for_zoom(zoom),
    };

    // Phase 1: lock, reset stale scene state, and claim at most one batch
    // reader. While it runs, camera motion is coalesced into the next repaint.
    let read = if let Ok(mut guard) = cache.lock() {
        if guard.scene_key.as_ref() != Some(&scene_key) {
            eprintln!(
                "[1kEE] scene change → {} assets for zoom_bucket={} (entries cleared)",
                assets.reader_assets().len(),
                assets
                    .reader_assets()
                    .first()
                    .map(|a| a.zoom_bucket)
                    .unwrap_or(-1)
            );
            guard.scene_key = Some(scene_key);
            guard.clear_entries_for_new_scene();
        }
        guard.last_status = Some(state.status);
        begin_local_read(
            &mut guard,
            assets.reader_assets(),
            center_lat_bucket,
            center_lon_bucket,
        )
    } else {
        None
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

    // Flattening the complete source envelope is intentionally asynchronous:
    // decode arrivals can contain thousands of paths, and cloning them on the
    // paint thread caused the visible tile-arrival hitch.
    let (contours, merge) = if let Ok(mut guard) = cache.lock() {
        retain_local_entries(&mut guard, center_lat_bucket, center_lon_bucket);
        let merge = begin_local_merge(
            &mut guard,
            LocalMergeSpec {
                center_lat_bucket,
                center_lon_bucket,
                source_radius: prefetch_radius,
                partition_bucket_step: None,
            },
        );
        (guard.merged.clone(), merge)
    } else {
        (None, None)
    };
    if let Some(work) = merge {
        spawn_local_merge(cache, work, ctx.clone(), "earth-local-contour-merge");
    }
    LocalContourLoad {
        contours,
        ready_buckets: state.ready_buckets,
        status: state.status,
    }
}

/// Lunar analogue of `load_srtm_region_for_view` — sources from SLDEM2015 tiles
/// stored in `lunar_focus_cache.sqlite`.  Same streaming / caching pattern.
pub fn load_lunar_region_for_view(
    selected_root: Option<&Path>,
    scene_anchor: crate::model::GeoPoint,
    viewport_center: crate::model::GeoPoint,
    zoom: f32,
    prefetch_radius: i32,
    build_radius: i32,
    ctx: egui::Context,
) -> LocalContourLoad {
    let prefetch_radius = prefetch_radius.clamp(0, 16);
    let build_radius = build_radius.clamp(0, prefetch_radius);

    let cache: &'static Mutex<LocalRegionCache> =
        LUNAR_LOCAL_CONTOUR_CACHE.get_or_init(|| Mutex::new(LocalRegionCache::default()));

    let current_zoom_bucket = srtm_focus_cache::zoom::lunar_spec_for_zoom(zoom).zoom_bucket;
    let bucket_step = srtm_focus_cache::lunar_half_extent_for_zoom(zoom) * 0.45;
    let center_lat_bucket = (viewport_center.lat / bucket_step).round() as i32;
    let center_lon_bucket = (viewport_center.lon / bucket_step).round() as i32;
    let manifest_key = LocalManifestKey {
        root: selected_root.map(Path::to_path_buf),
        center_lat_bucket,
        center_lon_bucket,
        zoom_bucket: current_zoom_bucket,
        prefetch_radius,
        build_radius,
    };
    let manifest_root = selected_root.map(Path::to_path_buf);
    let assets = local_manifest_assets(
        cache,
        manifest_key,
        ctx.clone(),
        "lunar-local-contour-manifest",
        move || {
            srtm_focus_cache::ensure_lunar_contour_region(
                manifest_root.as_deref(),
                viewport_center,
                zoom,
                prefetch_radius,
                build_radius,
            )
        },
    );
    let state = srtm_focus_cache::local_contour_region_state(
        crate::model::ActiveBody::Moon,
        assets.display_assets(),
        viewport_center,
        zoom,
        build_radius,
    );
    let per_asset_budget = (360usize / assets.reader_assets().len().max(1)).max(120);
    let scene_key = SceneKey {
        root: selected_root.map(Path::to_path_buf),
        anchor_lat_bucket: (scene_anchor.lat * 20.0).round() as i32,
        anchor_lon_bucket: (scene_anchor.lon * 20.0).round() as i32,
        zoom_bucket: current_zoom_bucket,
    };

    let read = if let Ok(mut guard) = cache.lock() {
        if guard.scene_key.as_ref() != Some(&scene_key) {
            // Scene changed (usually a zoom level change).  Build a fallback
            // draw window so the user keeps seeing the old resolution while
            // the new tiles are building. Do not clone the full retention
            // envelope just to create this temporary fallback.
            guard.zoom_fallback = guard.merged.clone();
            guard.scene_key = Some(scene_key);
            guard.clear_entries_for_new_scene();
        }
        guard.last_status = Some(state.status);
        (!assets.reader_assets().is_empty())
            .then(|| {
                begin_local_read(
                    &mut guard,
                    assets.reader_assets(),
                    center_lat_bucket,
                    center_lon_bucket,
                )
            })
            .flatten()
    } else {
        None
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

    let (contours, merge) = if let Ok(mut guard) = cache.lock() {
        retain_local_entries(&mut guard, center_lat_bucket, center_lon_bucket);
        let merge = begin_local_merge(
            &mut guard,
            LocalMergeSpec {
                center_lat_bucket,
                center_lon_bucket,
                source_radius: prefetch_radius,
                partition_bucket_step: Some(bucket_step),
            },
        );
        let contours = guard
            .merged
            .as_ref()
            .filter(|merged| !merged.is_empty())
            .cloned()
            .or_else(|| guard.zoom_fallback.clone());
        (contours, merge)
    } else {
        (None, None)
    };
    if let Some(work) = merge {
        spawn_local_merge(cache, work, ctx.clone(), "lunar-local-contour-merge");
    }
    LocalContourLoad {
        contours,
        ready_buckets: state.ready_buckets,
        status: state.status,
    }
}

/// Mars analogue of `load_srtm_region_for_view` — sources from MRO CTX tiles
/// stored in `mars_ctx_cache.sqlite`.
pub fn load_mars_region_for_view(
    selected_root: Option<&Path>,
    scene_anchor: crate::model::GeoPoint,
    viewport_center: crate::model::GeoPoint,
    zoom: f32,
    prefetch_radius: i32,
    build_radius: i32,
    ctx: egui::Context,
) -> LocalContourLoad {
    let prefetch_radius = prefetch_radius.clamp(0, 16);
    let build_radius = build_radius.clamp(0, prefetch_radius);

    let cache: &'static Mutex<LocalRegionCache> =
        MARS_LOCAL_CONTOUR_CACHE.get_or_init(|| Mutex::new(LocalRegionCache::default()));

    let current_zoom_bucket = srtm_focus_cache::zoom::mars_spec_for_zoom(zoom).zoom_bucket;
    let bucket_step = srtm_focus_cache::mars_half_extent_for_zoom(zoom) * 0.45;
    let center_lat_bucket = (viewport_center.lat / bucket_step).round() as i32;
    let center_lon_bucket = (viewport_center.lon / bucket_step).round() as i32;
    let manifest_key = LocalManifestKey {
        root: selected_root.map(Path::to_path_buf),
        center_lat_bucket,
        center_lon_bucket,
        zoom_bucket: current_zoom_bucket,
        prefetch_radius,
        build_radius,
    };
    let manifest_root = selected_root.map(Path::to_path_buf);
    let assets = local_manifest_assets(
        cache,
        manifest_key,
        ctx.clone(),
        "mars-local-contour-manifest",
        move || {
            srtm_focus_cache::ensure_mars_contour_region(
                manifest_root.as_deref(),
                viewport_center,
                zoom,
                prefetch_radius,
                build_radius,
            )
        },
    );
    let state = srtm_focus_cache::local_contour_region_state(
        crate::model::ActiveBody::Mars,
        assets.display_assets(),
        viewport_center,
        zoom,
        build_radius,
    );
    let per_asset_budget = (360usize / assets.reader_assets().len().max(1)).max(120);
    let scene_key = SceneKey {
        root: selected_root.map(Path::to_path_buf),
        anchor_lat_bucket: (scene_anchor.lat * 20.0).round() as i32,
        anchor_lon_bucket: (scene_anchor.lon * 20.0).round() as i32,
        zoom_bucket: current_zoom_bucket,
    };

    let read = if let Ok(mut guard) = cache.lock() {
        if guard.scene_key.as_ref() != Some(&scene_key) {
            // Preserve the previous draw window only; older retained tiles
            // remain decoded for a return pan but do not need cloning here.
            guard.zoom_fallback = guard.merged.clone();
            guard.scene_key = Some(scene_key);
            guard.clear_entries_for_new_scene();
        }
        guard.last_status = Some(state.status);
        (!assets.reader_assets().is_empty())
            .then(|| {
                begin_local_read(
                    &mut guard,
                    assets.reader_assets(),
                    center_lat_bucket,
                    center_lon_bucket,
                )
            })
            .flatten()
    } else {
        None
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

    let (contours, merge) = if let Ok(mut guard) = cache.lock() {
        retain_local_entries(&mut guard, center_lat_bucket, center_lon_bucket);
        let merge = begin_local_merge(
            &mut guard,
            LocalMergeSpec {
                center_lat_bucket,
                center_lon_bucket,
                source_radius: prefetch_radius,
                partition_bucket_step: Some(bucket_step),
            },
        );
        let contours = guard
            .merged
            .as_ref()
            .filter(|merged| !merged.is_empty())
            .cloned()
            .or_else(|| guard.zoom_fallback.clone());
        (contours, merge)
    } else {
        (None, None)
    };
    if let Some(work) = merge {
        spawn_local_merge(cache, work, ctx.clone(), "mars-local-contour-merge");
    }
    LocalContourLoad {
        contours,
        ready_buckets: state.ready_buckets,
        status: state.status,
    }
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

    let assets =
        srtm_focus_cache::ensure_focus_contour_region(selected_root, center, tile_zoom, 2, 2);

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

    let assets = srtm_focus_cache::ensure_lunar_contour_region(
        selected_root,
        center,
        tile_zoom,
        2,
        2,
    );

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

    let assets = srtm_focus_cache::ensure_mars_contour_region(
        selected_root,
        center,
        tile_zoom,
        2,
        2,
    );

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

    fn test_cache_key(lat_bucket: i32, lon_bucket: i32) -> CacheKey {
        CacheKey {
            path: PathBuf::from("test-cache.sqlite"),
            lat_bucket,
            lon_bucket,
            zoom_bucket: 0,
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
    fn local_draw_window_keeps_outer_tiles_decoded_for_return_pans() {
        let near = test_cache_key(0, 0);
        let outer = test_cache_key(5, 0);
        let mut cache = LocalRegionCache::default();
        cache
            .entries
            .insert(near.clone(), Arc::new(vec![test_contour(10.0)]));
        cache
            .entries
            .insert(outer.clone(), Arc::new(vec![test_contour(20.0)]));
        cache.mark_entries_changed();

        let visible = merged_local_contours(&mut cache, 0, 0, 4)
            .expect("near tile should be in the draw window");
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].elevation_m, 10.0);
        assert!(cache.entries.contains_key(&outer));

        let after_pan = merged_local_contours(&mut cache, 5, 0, 4)
            .expect("retained outer tile should render after return pan");
        assert_eq!(after_pan.len(), 1);
        assert_eq!(after_pan[0].elevation_m, 20.0);
        assert!(cache.entries.contains_key(&near));
    }

    #[test]
    fn partitioned_merge_keeps_an_outer_owner_that_crosses_the_viewport_edge() {
        let outer_owner = test_cache_key(5, 0);
        let owned_contour = ContourPath {
            elevation_m: 20.0,
            // The midpoint belongs to bucket 5, while the first point reaches
            // back into the camera's near-edge cull margin.
            points: vec![
                GeoPoint { lat: 0.5, lon: 0.0 },
                GeoPoint { lat: 5.0, lon: 0.0 },
                GeoPoint { lat: 5.1, lon: 0.1 },
            ],
        };
        let mut cache = LocalRegionCache::default();
        cache
            .entries
            .insert(outer_owner, Arc::new(vec![owned_contour]));
        cache.mark_entries_changed();

        assert!(merged_partitioned_local_contours(&mut cache, 1.0, 0, 0, 4).is_none());
        let prefetched = merged_partitioned_local_contours(&mut cache, 1.0, 0, 0, 6)
            .expect("full source window should retain the outer owner");
        assert_eq!(prefetched.len(), 1);
        assert_eq!(prefetched[0].elevation_m, 20.0);
    }

    #[test]
    fn background_local_merge_discards_a_stale_tile_snapshot() {
        let first = test_cache_key(0, 0);
        let second = test_cache_key(1, 0);
        let cache = Mutex::new(LocalRegionCache::default());
        let spec = LocalMergeSpec {
            center_lat_bucket: 0,
            center_lon_bucket: 0,
            source_radius: 2,
            partition_bucket_step: None,
        };

        let stale_work = {
            let mut state = cache.lock().expect("test cache lock");
            state
                .entries
                .insert(first, Arc::new(vec![test_contour(10.0)]));
            state.mark_entries_changed();
            begin_local_merge(&mut state, spec).expect("initial merge work")
        };
        let stale_key = stale_work.key;

        {
            let mut state = cache.lock().expect("test cache lock");
            state
                .entries
                .insert(second, Arc::new(vec![test_contour(20.0)]));
            state.mark_entries_changed();
            // A new camera/revision request is recorded, but waits for the
            // active worker rather than allowing concurrent full clones.
            assert!(begin_local_merge(&mut state, spec).is_none());
            assert_ne!(state.merge_requested_key, Some(stale_key));
        }

        finish_local_merge(&cache, stale_key, Some(flatten_local_merge(stale_work)));

        let mut state = cache.lock().expect("test cache lock");
        assert!(state.merged_key.is_none());
        assert!(state.merge_in_flight.is_none());
        let fresh_work = begin_local_merge(&mut state, spec).expect("fresh merge work");
        let fresh_key = fresh_work.key;
        drop(state);

        finish_local_merge(&cache, fresh_key, Some(flatten_local_merge(fresh_work)));
        let state = cache.lock().expect("test cache lock");
        let merged = state.merged.as_ref().expect("published current merge");
        assert_eq!(merged.len(), 2);
        assert_eq!(state.merged_key, Some(fresh_key));
    }

    #[test]
    fn stale_manifest_worker_cannot_replace_the_latest_window() {
        let stale_key = LocalManifestKey {
            root: None,
            center_lat_bucket: 0,
            center_lon_bucket: 0,
            zoom_bucket: 1,
            prefetch_radius: 6,
            build_radius: 6,
        };
        let requested_key = LocalManifestKey {
            center_lat_bucket: 1,
            ..stale_key.clone()
        };
        let cache = Mutex::new(LocalRegionCache {
            manifest_snapshot: Some(LocalManifestSnapshot {
                key: requested_key.clone(),
                assets: vec![test_asset(1, 0)],
                manifest_revision: 0,
                refreshed_at: Instant::now(),
            }),
            manifest_requested_key: Some(requested_key.clone()),
            manifest_in_flight: Some(stale_key.clone()),
            ..Default::default()
        });

        finish_local_manifest(&cache, &stale_key, Some(vec![test_asset(0, 0)]));

        let state = cache.lock().expect("test cache lock");
        assert!(state.manifest_in_flight.is_none());
        let snapshot = state.manifest_snapshot.as_ref().expect("current snapshot");
        assert!(snapshot.key == requested_key);
        assert_eq!(snapshot.assets[0].lat_bucket, 1);
    }

    #[test]
    fn compatible_manifest_pan_fallback_keeps_overlapping_tiles_ready() {
        let source_key = LocalManifestKey {
            root: Some(PathBuf::from("terrain-root")),
            center_lat_bucket: 0,
            center_lon_bucket: 0,
            zoom_bucket: 0,
            prefetch_radius: 1,
            build_radius: 1,
        };
        let snapshot = LocalManifestSnapshot {
            key: source_key.clone(),
            assets: (-1..=1)
                .flat_map(|lat_bucket| {
                    (-1..=1).map(move |lon_bucket| test_asset(lat_bucket, lon_bucket))
                })
                .collect(),
            manifest_revision: 0,
            refreshed_at: Instant::now(),
        };

        let exact = local_manifest_asset_views(Some(&snapshot), &source_key);
        assert_eq!(exact.reader_assets().len(), 9);
        assert_eq!(exact.display_assets().len(), 9);

        let adjacent_key = LocalManifestKey {
            center_lat_bucket: 1,
            ..source_key.clone()
        };
        let adjacent = local_manifest_asset_views(Some(&snapshot), &adjacent_key);
        // A changed viewport must not drive a reader from the old manifest,
        // but the 2×3 overlapping tile strip remains valid progress data.
        assert!(adjacent.reader_assets().is_empty());
        assert_eq!(adjacent.display_assets().len(), 6);
        assert!(adjacent
            .display_assets()
            .iter()
            .all(|asset| (0..=1).contains(&asset.lat_bucket)));

        let zoom = 0.5;
        let bucket_step = srtm_focus_cache::half_extent_for_zoom(zoom) * 0.45;
        let progress = srtm_focus_cache::local_contour_region_state(
            crate::model::ActiveBody::Earth,
            adjacent.display_assets(),
            GeoPoint {
                lat: bucket_step,
                lon: 0.0,
            },
            zoom,
            1,
        );
        assert_eq!(progress.status.ready_assets, 6);
        assert_eq!(progress.status.total_assets, 9);

        let different_root = LocalManifestKey {
            root: Some(PathBuf::from("other-root")),
            ..adjacent_key.clone()
        };
        let different_zoom = LocalManifestKey {
            zoom_bucket: 1,
            ..adjacent_key.clone()
        };
        let different_radii = LocalManifestKey {
            prefetch_radius: 2,
            ..adjacent_key
        };
        for incompatible_key in [different_root, different_zoom, different_radii] {
            let incompatible = local_manifest_asset_views(Some(&snapshot), &incompatible_key);
            assert!(incompatible.reader_assets().is_empty());
            assert!(incompatible.display_assets().is_empty());
        }
    }

    #[test]
    fn local_retention_window_prunes_only_distant_tiles() {
        let retained = test_cache_key(LOCAL_CONTOUR_RETAIN_RADIUS, 0);
        let distant = test_cache_key(LOCAL_CONTOUR_RETAIN_RADIUS + 1, 0);
        let mut cache = LocalRegionCache::default();
        cache
            .entries
            .insert(retained.clone(), Arc::new(vec![test_contour(10.0)]));
        cache
            .entries
            .insert(distant.clone(), Arc::new(vec![test_contour(20.0)]));
        cache.mark_entries_changed();

        retain_local_entries(&mut cache, 0, 0);

        assert!(cache.entries.contains_key(&retained));
        assert!(!cache.entries.contains_key(&distant));
    }

    #[test]
    fn local_reader_prioritizes_the_viewport_center_and_bounds_its_batch() {
        let assets: Vec<_> = (0..=LOCAL_CONTOUR_READ_BATCH_SIZE as i32)
            .map(|lat_bucket| test_asset(lat_bucket, 0))
            .collect();
        let mut cache = LocalRegionCache::default();

        let (_epoch, requests) =
            begin_local_read(&mut cache, &assets, 0, 0).expect("local read batch");

        assert_eq!(requests.len(), LOCAL_CONTOUR_READ_BATCH_SIZE);
        assert_eq!(requests[0].0.lat_bucket, 0);
        assert!(!requests.iter().any(|(key, _)| {
            key.lat_bucket == LOCAL_CONTOUR_READ_BATCH_SIZE as i32
        }));
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
