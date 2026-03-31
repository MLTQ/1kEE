use super::db::tile_exists;
use super::gdal::{build_focus_contours, build_lunar_contour_tile, shutdown_requested};
use super::zoom::spec_for_zoom;
use super::{FocusContourAsset, FocusContourSpec, GeoBounds, TileKey};
use crate::model::GeoPoint;
use rusqlite::Connection;
use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

fn max_background_builds() -> usize {
    let cpus = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    (cpus.saturating_sub(1)).clamp(2, 8)
}

fn active_build_slots() -> &'static AtomicUsize {
    static ACTIVE: OnceLock<AtomicUsize> = OnceLock::new();
    ACTIVE.get_or_init(|| AtomicUsize::new(0))
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
    connection: &Connection,
    spec: FocusContourSpec,
    lat_bucket: i32,
    lon_bucket: i32,
    bucket_step: f32,
) -> Option<FocusContourAsset> {
    use super::zoom::spec_for_zoom as _spec_for_zoom;
    let _ = _spec_for_zoom; // suppress unused import warning

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

    // Use the shared connection passed from ensure_focus_contour_region —
    // avoids opening a new SQLite connection (with WAL pragma overhead) per tile.
    if tile_exists(connection, tile).unwrap_or(false) {
        return Some(FocusContourAsset {
            path: cache_db_path.to_path_buf(),
            simplify_step: spec.simplify_step,
            zoom_bucket: spec.zoom_bucket,
            lat_bucket,
            lon_bucket,
        });
    }

    // Cache miss — need SRTM root to build on-demand; skip silently if unavailable.
    let srtm_root = srtm_root?;

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

    let srtm_root = srtm_root.to_path_buf();
    let cache_root = cache_root.to_path_buf();
    let cache_db_path = cache_db_path.to_path_buf();
    std::thread::spawn(move || {
        let _ = build_focus_contours(&srtm_root, &cache_root, &cache_db_path, tile, bounds, spec);
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
    connection: &Connection,
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

    if tile_exists(connection, tile).unwrap_or(false) {
        return Some(FocusContourAsset {
            path: cache_db_path.to_path_buf(),
            simplify_step: spec.simplify_step,
            zoom_bucket: spec.zoom_bucket,
            lat_bucket,
            lon_bucket,
        });
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
        let _ =
            build_lunar_contour_tile(&jp2_path, &cache_root, &cache_db_path, tile, bounds, spec);
        if let Ok(mut guard) = lunar_pending_set().lock() {
            guard.remove(&tile);
        }
        release_build_slot();
        crate::app::request_repaint();
    });

    None
}
