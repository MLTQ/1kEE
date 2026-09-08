use super::gdal::{
    bounds_have_srtm_source, build_focus_contours, build_lunar_contour_tile,
    build_mars_contour_tile, build_threedep_contours, shutdown_requested,
};
use super::zoom::spec_uses_threedep;
use super::{FocusContourAsset, FocusContourSpec, GeoBounds, TileKey};
use crate::model::GeoPoint;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const FAILED_BUILD_RETRY_DELAY: Duration = Duration::from_secs(5);
const MAX_FAILED_BUILD_RETRY_DELAY: Duration = Duration::from_secs(60);
const MAX_TRACKED_FAILED_BUILDS: usize = 1_024;
/// A long ocean pan can visit many sourceless buckets. Bound the memo and
/// rebuild it on overflow; re-checking costs a handful of `exists()` calls.
const MAX_TRACKED_SOURCELESS_TILES: usize = 4_096;
/// Interactive contour creation invokes CPU- and I/O-heavy GDAL work. Keep a
/// core free for egui/WGPU and one for readers/merges while a local 13×13
/// envelope is filling; the companion cache builder remains the fast path for
/// bulk precomputation.
const MAX_INTERACTIVE_BACKGROUND_BUILDS: usize = 2;

#[derive(Clone, Copy)]
struct FailedBuildBackoff {
    retry_at: Instant,
    attempts: u8,
}

fn max_background_builds() -> usize {
    let cpus = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    cpus.saturating_sub(1)
        .clamp(1, MAX_INTERACTIVE_BACKGROUND_BUILDS)
}

fn active_build_slots() -> &'static AtomicUsize {
    static ACTIVE: OnceLock<AtomicUsize> = OnceLock::new();
    ACTIVE.get_or_init(|| AtomicUsize::new(0))
}

fn manifest_revision_counter() -> &'static AtomicU64 {
    static MANIFEST_REVISION: OnceLock<AtomicU64> = OnceLock::new();
    MANIFEST_REVISION.get_or_init(|| AtomicU64::new(0))
}

/// Monotonically increases after an in-process contour builder finishes. Local
/// manifest snapshots use it to refresh immediately instead of waiting for
/// their short external-writer timeout.
pub fn manifest_revision() -> u64 {
    manifest_revision_counter().load(Ordering::Acquire)
}

fn bump_manifest_revision() {
    manifest_revision_counter().fetch_add(1, Ordering::AcqRel);
}

pub fn try_acquire_build_slot() -> bool {
    let limit = max_background_builds();
    active_build_slots()
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |count| {
            (count < limit).then_some(count + 1)
        })
        .is_ok()
}

pub fn release_build_slot() {
    let current = active_build_slots().load(Ordering::SeqCst);
    if current > 0 {
        active_build_slots().fetch_sub(1, Ordering::SeqCst);
    }
}

pub fn pending_set() -> &'static Mutex<HashSet<TileKey>> {
    static PENDING: OnceLock<Mutex<HashSet<TileKey>>> = OnceLock::new();
    PENDING.get_or_init(|| Mutex::new(HashSet::new()))
}

pub fn lunar_pending_set() -> &'static Mutex<HashSet<TileKey>> {
    static LUNAR_PENDING: OnceLock<Mutex<HashSet<TileKey>>> = OnceLock::new();
    LUNAR_PENDING.get_or_init(|| Mutex::new(HashSet::new()))
}

pub fn mars_pending_set() -> &'static Mutex<HashSet<TileKey>> {
    static MARS_PENDING: OnceLock<Mutex<HashSet<TileKey>>> = OnceLock::new();
    MARS_PENDING.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Earth buckets whose footprint contains no SRTM source file at all.
///
/// SRTM covers land only, so an open-ocean bucket can never yield contours.
/// The set is scoped to the source root that produced it: pointing the app at
/// a different (or newly mounted) terrain root re-checks every bucket.
#[derive(Default)]
struct SourcelessTiles {
    root: Option<PathBuf>,
    tiles: HashSet<TileKey>,
}

fn sourceless_tiles() -> &'static Mutex<SourcelessTiles> {
    static SOURCELESS: OnceLock<Mutex<SourcelessTiles>> = OnceLock::new();
    SOURCELESS.get_or_init(|| Mutex::new(SourcelessTiles::default()))
}

/// Deep-tier buckets where 3DEP publishes no 1 m source. Kept apart from the
/// SRTM memo because that one is invalidated per source root, while 3DEP
/// coverage is a property of the service rather than a local directory.
fn threedep_uncovered_tiles() -> &'static Mutex<HashSet<TileKey>> {
    static UNCOVERED: OnceLock<Mutex<HashSet<TileKey>>> = OnceLock::new();
    UNCOVERED.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Record a deep-tier bucket that 3DEP cannot serve at 1 m. Only a resolved
/// negative probe reaches here; an unprobed bucket stays outstanding so it is
/// retried once its probe lands.
fn record_threedep_uncovered(tile: TileKey) {
    if let Ok(mut guard) = threedep_uncovered_tiles().lock() {
        if guard.len() >= MAX_TRACKED_SOURCELESS_TILES {
            guard.clear();
        }
        guard.insert(tile);
    }
}

/// Tiles known to have no source terrain, from either SRTM's local mirror or
/// 3DEP's published coverage. Region state treats them as resolved terrain
/// rather than tiles that are still loading.
pub fn sourceless_tile_set() -> HashSet<TileKey> {
    let mut tiles = sourceless_tiles()
        .lock()
        .map(|guard| guard.tiles.clone())
        .unwrap_or_default();
    if let Ok(guard) = threedep_uncovered_tiles().lock() {
        tiles.extend(guard.iter().copied());
    }
    tiles
}

/// Memoized "no source file overlaps this bucket" check.
///
/// Returning `true` is a permanent answer for the current source root, so the
/// caller must schedule no build and record no failure.
fn tile_lacks_srtm_source(srtm_root: &Path, tile: TileKey, bounds: GeoBounds) -> bool {
    if let Ok(guard) = sourceless_tiles().lock() {
        if guard.root.as_deref() == Some(srtm_root) && guard.tiles.contains(&tile) {
            return true;
        }
    }

    if bounds_have_srtm_source(srtm_root, bounds) {
        return false;
    }

    if let Ok(mut guard) = sourceless_tiles().lock() {
        if guard.root.as_deref() != Some(srtm_root) {
            guard.root = Some(srtm_root.to_path_buf());
            guard.tiles.clear();
        }
        if guard.tiles.len() >= MAX_TRACKED_SOURCELESS_TILES {
            guard.tiles.clear();
        }
        guard.tiles.insert(tile);
    }
    true
}

/// Forget which buckets lacked source terrain. A manual cache reset happens
/// after a user fixes storage or source availability, so a bucket that was
/// sourceless because its volume was unmounted must be re-checked.
pub fn clear_sourceless_tiles() {
    if let Ok(mut guard) = threedep_uncovered_tiles().lock() {
        guard.clear();
    }
    if let Ok(mut guard) = sourceless_tiles().lock() {
        guard.root = None;
        guard.tiles.clear();
    }
}

fn failed_builds() -> &'static Mutex<HashMap<TileKey, FailedBuildBackoff>> {
    static FAILED: OnceLock<Mutex<HashMap<TileKey, FailedBuildBackoff>>> = OnceLock::new();
    FAILED.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lunar_failed_builds() -> &'static Mutex<HashMap<TileKey, FailedBuildBackoff>> {
    static FAILED: OnceLock<Mutex<HashMap<TileKey, FailedBuildBackoff>>> = OnceLock::new();
    FAILED.get_or_init(|| Mutex::new(HashMap::new()))
}

fn mars_failed_builds() -> &'static Mutex<HashMap<TileKey, FailedBuildBackoff>> {
    static FAILED: OnceLock<Mutex<HashMap<TileKey, FailedBuildBackoff>>> = OnceLock::new();
    FAILED.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A failed cache volume or unavailable GDAL source affects more than one
/// tile. Per-body gates prevent a fresh outer ring from immediately consuming
/// every build slot after the first failure.
fn failed_build_gate() -> &'static Mutex<Option<FailedBuildBackoff>> {
    static FAILED: OnceLock<Mutex<Option<FailedBuildBackoff>>> = OnceLock::new();
    FAILED.get_or_init(|| Mutex::new(None))
}

fn lunar_failed_build_gate() -> &'static Mutex<Option<FailedBuildBackoff>> {
    static FAILED: OnceLock<Mutex<Option<FailedBuildBackoff>>> = OnceLock::new();
    FAILED.get_or_init(|| Mutex::new(None))
}

fn mars_failed_build_gate() -> &'static Mutex<Option<FailedBuildBackoff>> {
    static FAILED: OnceLock<Mutex<Option<FailedBuildBackoff>>> = OnceLock::new();
    FAILED.get_or_init(|| Mutex::new(None))
}

fn next_failed_build_backoff(previous: Option<FailedBuildBackoff>) -> FailedBuildBackoff {
    let attempts = previous
        .map(|backoff| backoff.attempts.saturating_add(1))
        .unwrap_or(1);
    let shift = attempts.saturating_sub(1).min(4);
    let delay = Duration::from_secs(
        (FAILED_BUILD_RETRY_DELAY.as_secs() << shift).min(MAX_FAILED_BUILD_RETRY_DELAY.as_secs()),
    );
    FailedBuildBackoff {
        retry_at: Instant::now() + delay,
        attempts,
    }
}

fn retry_allowed(failures: &Mutex<HashMap<TileKey, FailedBuildBackoff>>, tile: TileKey) -> bool {
    let Ok(failures) = failures.lock() else {
        return true;
    };
    let Some(backoff) = failures.get(&tile).copied() else {
        return true;
    };
    backoff.retry_at <= Instant::now()
}

fn record_failed_build(failures: &Mutex<HashMap<TileKey, FailedBuildBackoff>>, tile: TileKey) {
    if let Ok(mut failures) = failures.lock() {
        if failures.len() >= MAX_TRACKED_FAILED_BUILDS {
            let now = Instant::now();
            failures.retain(|_, backoff| backoff.retry_at > now);
        }
        let backoff = next_failed_build_backoff(failures.get(&tile).copied());
        failures.insert(tile, backoff);
    }
}

fn retry_gate_allows(gate: &Mutex<Option<FailedBuildBackoff>>) -> bool {
    gate.lock()
        .map(|gate| {
            gate.as_ref()
                .map_or(true, |backoff| backoff.retry_at <= Instant::now())
        })
        .unwrap_or(true)
}

fn record_failed_build_gate(gate: &Mutex<Option<FailedBuildBackoff>>) {
    if let Ok(mut gate) = gate.lock() {
        if gate
            .as_ref()
            .map_or(false, |backoff| backoff.retry_at > Instant::now())
        {
            return;
        }
        *gate = Some(next_failed_build_backoff(*gate));
    }
}

fn clear_failed_build_gate(gate: &Mutex<Option<FailedBuildBackoff>>) {
    if let Ok(mut gate) = gate.lock() {
        *gate = None;
    }
}

fn clear_failed_build(
    failures: &Mutex<HashMap<TileKey, FailedBuildBackoff>>,
    tile: TileKey,
) {
    if let Ok(mut failures) = failures.lock() {
        failures.remove(&tile);
    }
}

/// A manual tile-cache reset should also release failed-build cooldowns so a
/// user who fixed a missing source or freed storage can retry immediately.
pub fn clear_failed_build_backoffs() {
    clear_sourceless_tiles();
    for failures in [failed_builds(), lunar_failed_builds(), mars_failed_builds()] {
        if let Ok(mut failures) = failures.lock() {
            failures.clear();
        }
    }
    for gate in [
        failed_build_gate(),
        lunar_failed_build_gate(),
        mars_failed_build_gate(),
    ] {
        clear_failed_build_gate(gate);
    }
}

/// Maximum number of concurrent SLDEM2015 JP2 tile builds.
/// The JP2 is a single ~22 GB file; all lunar builds compete for the same I/O.
/// Capping at 2 keeps throughput high without thrashing disk/memory bandwidth.
pub const MAX_CONCURRENT_LUNAR_BUILDS: usize = 2;

pub fn is_pending(tile: TileKey) -> bool {
    pending_set()
        .lock()
        .map(|guard| guard.contains(&tile))
        .unwrap_or(false)
}

pub fn ensure_bucket_asset(
    srtm_root: Option<&Path>,
    cache_root: &Path,
    cache_db_path: &Path,
    manifest_contour_count: Option<i64>,
    allow_build: bool,
    spec: FocusContourSpec,
    lat_bucket: i32,
    lon_bucket: i32,
    bucket_step: f32,
) -> Option<FocusContourAsset> {
    if shutdown_requested().load(Ordering::Relaxed) {
        return None;
    }

    let bucket_center = GeoPoint {
        lat: (lat_bucket as f32 * bucket_step).clamp(-89.999, 89.999),
        lon: lon_bucket as f32 * bucket_step,
    };
    let bounds = GeoBounds::around(bucket_center, spec.half_extent_deg);
    let tile = TileKey {
        zoom_bucket: spec.zoom_bucket,
        lat_bucket,
        lon_bucket,
    };

    // Region selection supplies a single-query manifest snapshot. Any cached
    // manifest row is immediately renderable, including an explicitly empty
    // Earth tile.
    if manifest_contour_count.is_some() {
        clear_failed_build(failed_builds(), tile);
        return Some(FocusContourAsset {
            path: cache_db_path.to_path_buf(),
            simplify_step: spec.simplify_step,
            zoom_bucket: spec.zoom_bucket,
            lat_bucket,
            lon_bucket,
        });
    }

    if !allow_build {
        return None;
    }

    // The deep tiers source USGS 3DEP instead of SRTM, whose ~30 m posting
    // cannot support their sub-5 m intervals.
    let uses_threedep = spec_uses_threedep(&spec);

    // A bucket over open ocean has no SRTM source file to contour. That is a
    // permanent property of the terrain, not a build failure: schedule nothing,
    // record no cooldown, and let region state report the tile as resolved so
    // its loading pulse stops instead of cycling forever.
    if !uses_threedep && srtm_root.is_some_and(|root| tile_lacks_srtm_source(root, tile, bounds)) {
        return None;
    }

    // 3DEP only publishes 1 m coverage for parts of the United States. An
    // uncovered bucket is the same kind of permanent absence as open ocean, and
    // an unprobed one resolves on a later frame once its probe lands.
    if uses_threedep {
        match crate::threedep::coverage_at(bucket_center) {
            crate::threedep::Coverage::Fine => {}
            crate::threedep::Coverage::Coarse => {
                record_threedep_uncovered(tile);
                return None;
            }
            // Probe still in flight: leave the bucket outstanding so the next
            // frame after it resolves can schedule the build.
            crate::threedep::Coverage::Unknown => return None,
        }
    }

    if !retry_gate_allows(failed_build_gate()) || !retry_allowed(failed_builds(), tile) {
        return None;
    }

    // Cache miss — SRTM tiers need their root to build on-demand; skip silently
    // if unavailable. 3DEP tiers stream their source and need no local root.
    let srtm_root = if uses_threedep { None } else { Some(srtm_root?) };

    if is_pending(tile) {
        return None;
    }

    if !try_acquire_build_slot() {
        return None;
    }

    let pending = pending_set();
    let mut guard = pending.lock().ok()?;
    if !guard.insert(tile) {
        release_build_slot();
        return None;
    }
    drop(guard);

    let srtm_root = srtm_root.map(|root| root.to_path_buf());
    let cache_root = cache_root.to_path_buf();
    let cache_db_path = cache_db_path.to_path_buf();
    std::thread::spawn(move || {
        let built = match srtm_root.as_deref() {
            Some(root) => {
                build_focus_contours(root, &cache_root, &cache_db_path, tile, bounds, spec)
            }
            None => build_threedep_contours(&cache_root, &cache_db_path, tile, bounds, spec),
        };
        if built.is_some() {
            bump_manifest_revision();
            clear_failed_build(failed_builds(), tile);
            clear_failed_build_gate(failed_build_gate());
        } else {
            record_failed_build(failed_builds(), tile);
            record_failed_build_gate(failed_build_gate());
        }
        if let Ok(mut guard) = pending_set().lock() {
            guard.remove(&tile);
        }
        release_build_slot();
        crate::app::request_repaint();
    });

    None
}

/// Lunar analogue of `ensure_bucket_asset` — sources from a single SLDEM2015
/// JP2 file instead of a directory of SRTM tiles.
pub fn ensure_lunar_bucket_asset(
    jp2_path: &Path,
    cache_root: &Path,
    cache_db_path: &Path,
    manifest_contour_count: Option<i64>,
    allow_build: bool,
    spec: FocusContourSpec,
    lat_bucket: i32,
    lon_bucket: i32,
    bucket_step: f32,
) -> Option<FocusContourAsset> {
    if shutdown_requested().load(Ordering::Relaxed) {
        return None;
    }

    let bucket_center = GeoPoint {
        lat: (lat_bucket as f32 * bucket_step).clamp(-89.999, 89.999),
        lon: lon_bucket as f32 * bucket_step,
    };
    let bounds = GeoBounds::around(bucket_center, spec.half_extent_deg);
    let tile = TileKey {
        zoom_bucket: spec.zoom_bucket,
        lat_bucket,
        lon_bucket,
    };

    if manifest_contour_count.is_some() {
        clear_failed_build(lunar_failed_builds(), tile);
        return Some(FocusContourAsset {
            path: cache_db_path.to_path_buf(),
            simplify_step: spec.simplify_step,
            zoom_bucket: spec.zoom_bucket,
            lat_bucket,
            lon_bucket,
        });
    }

    if !allow_build {
        return None;
    }

    if !retry_gate_allows(lunar_failed_build_gate()) || !retry_allowed(lunar_failed_builds(), tile) {
        return None;
    }

    // All lunar builds read from the same large JP2 file — cap concurrency to
    // avoid I/O starvation.  Use the pending set's current size as ground truth
    // so the limit is always accurate regardless of thread scheduling.
    let pending = lunar_pending_set();
    {
        let guard = pending.lock().ok()?;
        if guard.contains(&tile) {
            return None; // already in-flight
        }
        if guard.len() >= MAX_CONCURRENT_LUNAR_BUILDS {
            return None; // at concurrency limit
        }
    }

    if !try_acquire_build_slot() {
        return None; // also respect the global SRTM/misc slot budget
    }

    let mut guard = pending.lock().ok()?;
    if !guard.insert(tile) {
        release_build_slot();
        return None;
    }
    drop(guard);

    let jp2_path = jp2_path.to_path_buf();
    let cache_root = cache_root.to_path_buf();
    let cache_db_path = cache_db_path.to_path_buf();
    std::thread::spawn(move || {
        if build_lunar_contour_tile(&jp2_path, &cache_root, &cache_db_path, tile, bounds, spec)
            .is_some()
        {
            bump_manifest_revision();
            clear_failed_build(lunar_failed_builds(), tile);
            clear_failed_build_gate(lunar_failed_build_gate());
        } else {
            record_failed_build(lunar_failed_builds(), tile);
            record_failed_build_gate(lunar_failed_build_gate());
        }
        if let Ok(mut guard) = lunar_pending_set().lock() {
            guard.remove(&tile);
        }
        release_build_slot();
        crate::app::request_repaint();
    });

    None
}

/// Mars analogue of `ensure_bucket_asset` — queries the spatial index for
/// CTX DTM tiles covering this bucket and warps them to Mars longlat on demand.
/// When `mola_tiles` is non-empty it is used as a global fallback for regions
/// with no CTX coverage, and any previously cached 0-count (empty) tiles are
/// rebuilt from MOLA on the next call.
pub fn ensure_mars_bucket_asset(
    data_root: &Path,
    mola_tiles: &[std::path::PathBuf],
    cache_root: &Path,
    cache_db_path: &Path,
    manifest_contour_count: Option<i64>,
    allow_build: bool,
    spec: FocusContourSpec,
    lat_bucket: i32,
    lon_bucket: i32,
    bucket_step: f32,
) -> Option<FocusContourAsset> {
    if shutdown_requested().load(Ordering::Relaxed) {
        return None;
    }

    let bucket_center = GeoPoint {
        lat: (lat_bucket as f32 * bucket_step).clamp(-89.999, 89.999),
        lon: lon_bucket as f32 * bucket_step,
    };
    let bounds = GeoBounds::around(bucket_center, spec.half_extent_deg);
    let tile = TileKey {
        zoom_bucket: spec.zoom_bucket,
        lat_bucket,
        lon_bucket,
    };

    // Check the cache.  A tile with contour_count > 0 is fully built — serve it.
    // A tile with contour_count = 0 was previously marked empty (no CTX coverage);
    // if MOLA is now available we fall through and rebuild it.
    match manifest_contour_count {
        Some(count) if count > 0 => {
            clear_failed_build(mars_failed_builds(), tile);
            return Some(FocusContourAsset {
                path: cache_db_path.to_path_buf(),
                simplify_step: spec.simplify_step,
                zoom_bucket: spec.zoom_bucket,
                lat_bucket,
                lon_bucket,
            });
        }
        Some(0) if mola_tiles.is_empty() => {
            // Cached empty and no MOLA to upgrade it — serve the empty asset
            // so the render loop doesn't retry this tile every frame.
            clear_failed_build(mars_failed_builds(), tile);
            return Some(FocusContourAsset {
                path: cache_db_path.to_path_buf(),
                simplify_step: spec.simplify_step,
                zoom_bucket: spec.zoom_bucket,
                lat_bucket,
                lon_bucket,
            });
        }
        // Some(0) with MOLA available, or None (not cached yet) — fall through to build.
        _ => {}
    }

    if !allow_build {
        return None;
    }

    if !retry_gate_allows(mars_failed_build_gate()) || !retry_allowed(mars_failed_builds(), tile) {
        return None;
    }

    let pending = mars_pending_set();
    {
        let guard = pending.lock().ok()?;
        if guard.contains(&tile) {
            return None; // already in-flight
        }
        if guard.len() >= MAX_CONCURRENT_LUNAR_BUILDS {
            return None; // same concurrency cap as lunar
        }
    }

    if !try_acquire_build_slot() {
        return None;
    }

    let mut guard = pending.lock().ok()?;
    if !guard.insert(tile) {
        release_build_slot();
        return None;
    }
    drop(guard);

    let data_root = data_root.to_path_buf();
    let mola_tiles = mola_tiles.to_vec();
    let cache_root = cache_root.to_path_buf();
    let cache_db_path = cache_db_path.to_path_buf();
    std::thread::spawn(move || {
        if build_mars_contour_tile(
            &data_root, &mola_tiles, &cache_root, &cache_db_path, tile, bounds, spec,
        )
        .is_some()
        {
            bump_manifest_revision();
            clear_failed_build(mars_failed_builds(), tile);
            clear_failed_build_gate(mars_failed_build_gate());
        } else {
            record_failed_build(mars_failed_builds(), tile);
            record_failed_build_gate(mars_failed_build_gate());
        }
        if let Ok(mut guard) = mars_pending_set().lock() {
            guard.remove(&tile);
        }
        release_build_slot();
        crate::app::request_repaint();
    });

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_build_cooldown_blocks_only_until_it_is_cleared() {
        let failures = Mutex::new(HashMap::new());
        let gate = Mutex::new(None);
        let tile = TileKey {
            zoom_bucket: 2,
            lat_bucket: 3,
            lon_bucket: -4,
        };

        assert!(retry_allowed(&failures, tile));
        record_failed_build(&failures, tile);
        assert!(!retry_allowed(&failures, tile));
        record_failed_build_gate(&gate);
        assert!(!retry_gate_allows(&gate));
        clear_failed_build(&failures, tile);
        clear_failed_build_gate(&gate);
        assert!(retry_allowed(&failures, tile));
        assert!(retry_gate_allows(&gate));
    }
}
