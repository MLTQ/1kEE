//! Keep source detail independent of the camera's continuous zoom.
use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

/// Finest SRTM tier. Keep this cache alive across all hosted zoom tiers, and
/// share it with ordinary bucket-six viewing so crossing the boundary is warm.
const BASE_ZOOM: f32 = 12.0;
pub(super) const BASE_BUCKET: i32 = 6;
static PENDING: AtomicBool = AtomicBool::new(true);

pub(super) fn pending() -> bool {
    PENDING.load(Ordering::Relaxed)
}

pub(super) fn reset() {
    PENDING.store(true, Ordering::Relaxed);
}

pub(super) fn load(
    root: Option<&Path>,
    anchor: GeoPoint,
    center: GeoPoint,
    view_zoom: f32,
    prefetch_radius: i32,
    build_radius: i32,
    ctx: egui::Context,
) -> LocalContourLoad {
    let bucket = srtm_focus_cache::zoom_bucket_for_zoom(view_zoom);
    let cache = if bucket == BASE_BUCKET {
        &EARTH_BASE_CONTOUR_CACHE
    } else {
        &LOCAL_CONTOUR_CACHE
    };
    // Always try the requested tier, including persisted fine tiles when
    // streaming is disabled or a coverage probe has no answer. No network
    // availability decision is allowed to hide an existing offline tile.
    let primary = load_earth_source_region(
        cache.get_or_init(|| Mutex::new(LocalRegionCache::default())),
        root,
        anchor,
        center,
        view_zoom,
        prefetch_radius,
        build_radius,
        ctx.clone(),
    );
    let primary_pending = is_pending(&primary);
    if bucket <= BASE_BUCKET || covers_view(&primary, center, view_zoom) {
        PENDING.store(primary_pending, Ordering::Relaxed);
        return primary;
    }

    // Request the same ground area at the base tier, not the much wider area
    // corresponding to BASE_ZOOM's camera. This also handles a cold deep start
    // and pans into areas that have never had fine tiles.
    let radius = base_radius(view_zoom);
    let base = load_earth_source_region(
        EARTH_BASE_CONTOUR_CACHE.get_or_init(|| Mutex::new(LocalRegionCache::default())),
        root,
        anchor,
        center,
        BASE_ZOOM,
        radius,
        radius,
        ctx,
    );
    PENDING.store(primary_pending || is_pending(&base), Ordering::Relaxed);
    choose(primary, base)
}

fn is_pending(load: &LocalContourLoad) -> bool {
    load.status.ready_assets < load.status.total_assets
}

fn visible_half_extent(zoom: f32) -> f32 {
    super::super::local_terrain_scene::visual_half_extent_for_zoom(zoom)
        * srtm_focus_cache::OBLIQUE_VISIBLE_EXTENT_FACTOR
}

fn base_radius(view_zoom: f32) -> i32 {
    let step = srtm_focus_cache::half_extent_for_zoom(BASE_ZOOM) * 0.45;
    (visible_half_extent(view_zoom) / step)
        .ceil()
        .clamp(1.0, 16.0) as i32
}

/// A count of "ready" assets is insufficient: unavailable hosted cells are
/// deliberately counted as complete by the progress UI. Require decoded tiles
/// covering the actual view before replacing its base terrain. Ignore the
/// extra prefetch ring, and accept decoded empty tiles (ocean/flat terrain).
fn covers_view(load: &LocalContourLoad, center: GeoPoint, view_zoom: f32) -> bool {
    let bucket = srtm_focus_cache::zoom_bucket_for_zoom(load.source_zoom);
    let step = srtm_focus_cache::half_extent_for_zoom(load.source_zoom) * 0.45;
    let half = visible_half_extent(view_zoom);
    let lat_min = ((center.lat - half) / step).round() as i32;
    let lat_max = ((center.lat + half) / step).round() as i32;
    let lon_min = ((center.lon - half) / step).round() as i32;
    let lon_max = ((center.lon + half) / step).round() as i32;
    let decoded: HashSet<_> = load
        .tiles
        .iter()
        .filter(|tile| tile.id.zoom_bucket == bucket)
        .filter(|tile| {
            load.ready_buckets
                .contains(&(tile.id.lat_bucket, tile.id.lon_bucket))
        })
        .map(|tile| (tile.id.lat_bucket, tile.id.lon_bucket))
        .collect();
    (lat_min..=lat_max).all(|lat| (lon_min..=lon_max).all(|lon| decoded.contains(&(lat, lon))))
}

fn has_geometry(load: &LocalContourLoad) -> bool {
    load.tiles.iter().any(|tile| !tile.contours.is_empty())
        || load
            .contours
            .as_ref()
            .is_some_and(|lines| !lines.is_empty())
}

fn choose(primary: LocalContourLoad, base: LocalContourLoad) -> LocalContourLoad {
    // Display an available base instead of a fine patch surrounded by holes.
    // If there is no base data, continue showing fine arrivals progressively.
    // Returning the entire load keeps progress, grid coordinates, GPU IDs and
    // CPU/elevation-fill geometry on one consistent source tier.
    if has_geometry(&base) || !has_geometry(&primary) {
        base
    } else {
        primary
    }
}

#[cfg(test)]
#[path = "contour_fallback_tests.rs"]
mod tests;
