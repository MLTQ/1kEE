use crate::model::GeoPoint;
use crate::terrain_assets;
use rusqlite::Connection;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::Ordering;
use std::time::Duration;

pub mod builders;
pub mod db;
pub mod gdal;
pub mod zoom;

pub use zoom::{
    bucket_radius_for_target_radius_miles, contour_interval_for_zoom, feature_budget_for_zoom,
    half_extent_for_zoom, zoom_bucket_for_zoom,
};

const BUILD_TIMEOUT: Duration = Duration::from_secs(90);
const CACHE_DB_NAME: &str = "srtm_focus_cache.sqlite";
const TEMP_DIR_NAME: &str = "srtm_focus_tmp";

#[derive(Clone)]
pub struct FocusContourAsset {
    pub path: PathBuf,
    pub simplify_step: usize,
    pub zoom_bucket: i32,
    pub lat_bucket: i32,
    pub lon_bucket: i32,
}

#[derive(Clone, Copy)]
pub struct FocusContourRegionStatus {
    pub ready_assets: usize,
    pub pending_assets: usize,
    pub total_assets: usize,
}

#[derive(Clone, Copy)]
pub(self) struct FocusContourSpec {
    pub half_extent_deg: f32,
    pub raster_size: u32,
    pub interval_m: i32,
    pub simplify_step: usize,
    pub feature_budget: usize,
    pub zoom_bucket: i32,
}

#[derive(Clone, Copy)]
pub(self) struct GeoBounds {
    pub min_lat: f32,
    pub max_lat: f32,
    pub min_lon: f32,
    pub max_lon: f32,
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub(self) struct TileKey {
    pub zoom_bucket: i32,
    pub lat_bucket: i32,
    pub lon_bucket: i32,
}

pub fn ensure_focus_contour_region(
    selected_root: Option<&Path>,
    focus: GeoPoint,
    zoom: f32,
    radius: i32,
) -> Vec<FocusContourAsset> {
    ensure_focus_contour_region_inner(selected_root, focus, zoom, radius, true)
}

/// Cache-only variant: never spawns on-demand GDAL builds for uncached tiles.
/// Used by the globe view, where tiles are large and on-demand builds are slow.
pub fn ensure_focus_contour_region_cached(
    selected_root: Option<&Path>,
    focus: GeoPoint,
    zoom: f32,
    radius: i32,
) -> Vec<FocusContourAsset> {
    ensure_focus_contour_region_inner(selected_root, focus, zoom, radius, false)
}

fn ensure_focus_contour_region_inner(
    selected_root: Option<&Path>,
    focus: GeoPoint,
    zoom: f32,
    radius: i32,
    build_missing: bool,
) -> Vec<FocusContourAsset> {
    // SRTM root is only needed to spawn on-demand GDAL builds for uncached tiles.
    // Pre-built tiles in the SQLite cache are returned even without SRTM access.
    let srtm_root = if build_missing {
        terrain_assets::find_srtm_root(selected_root)
    } else {
        None
    };
    let Some(cache_root) = db::focus_cache_root(selected_root) else {
        return Vec::new();
    };
    let Some(cache_db_path) = db::focus_cache_db_path(selected_root) else {
        return Vec::new();
    };
    // Open ONE connection for all tile checks — avoids 25 separate open+pragma
    // cycles per frame that stall the render thread under WAL contention.
    let Ok(connection) = db::open_cache_db(&cache_db_path) else {
        return Vec::new();
    };
    let spec = zoom::spec_for_zoom(zoom);
    let bucket_step = spec.half_extent_deg * 0.45;
    let center_lat_bucket = (focus.lat / bucket_step).round() as i32;
    let center_lon_bucket = (focus.lon / bucket_step).round() as i32;
    let mut assets = Vec::new();

    for lat_bucket in (center_lat_bucket - radius)..=(center_lat_bucket + radius) {
        for lon_bucket in (center_lon_bucket - radius)..=(center_lon_bucket + radius) {
            if let Some(asset) = builders::ensure_bucket_asset(
                srtm_root.as_deref(),
                &cache_root,
                &cache_db_path,
                &connection,
                spec,
                lat_bucket,
                lon_bucket,
                bucket_step,
            ) {
                assets.push(asset);
            }
        }
    }

    assets
}

/// Returns the set of `(lat_bucket, lon_bucket)` pairs that are already built
/// in the cache DB for the given zoom level.  Used by the pulse-grid renderer
/// to skip drawing placeholder rectangles over tiles that are already ready.
pub fn ready_tile_buckets(
    selected_root: Option<&Path>,
    focus: GeoPoint,
    zoom: f32,
    radius: i32,
) -> HashSet<(i32, i32)> {
    let mut set = HashSet::new();
    let Some(cache_db_path) = db::focus_cache_db_path(selected_root) else {
        return set;
    };
    let Ok(connection) = db::open_cache_db(&cache_db_path) else {
        return set;
    };
    let spec = zoom::spec_for_zoom(zoom);
    let bucket_step = spec.half_extent_deg * 0.45;
    let center_lat_bucket = (focus.lat / bucket_step).round() as i32;
    let center_lon_bucket = (focus.lon / bucket_step).round() as i32;
    for lat_bucket in (center_lat_bucket - radius)..=(center_lat_bucket + radius) {
        for lon_bucket in (center_lon_bucket - radius)..=(center_lon_bucket + radius) {
            let tile = TileKey {
                zoom_bucket: spec.zoom_bucket,
                lat_bucket,
                lon_bucket,
            };
            if db::tile_exists(&connection, tile).unwrap_or(false) {
                set.insert((lat_bucket, lon_bucket));
            }
        }
    }
    set
}

pub fn focus_contour_region_status(
    selected_root: Option<&Path>,
    focus: GeoPoint,
    zoom: f32,
    radius: i32,
) -> Option<FocusContourRegionStatus> {
    // Don't require SRTM root — status should reflect cache hits too.
    let cache_db_path = db::focus_cache_db_path(selected_root)?;
    let connection = db::open_cache_db(&cache_db_path).ok()?;
    let spec = zoom::spec_for_zoom(zoom);
    let bucket_step = spec.half_extent_deg * 0.45;
    let center_lat_bucket = (focus.lat / bucket_step).round() as i32;
    let center_lon_bucket = (focus.lon / bucket_step).round() as i32;
    let mut ready_assets = 0usize;
    let mut pending_assets = 0usize;
    let total_assets = ((radius * 2 + 1) as usize).pow(2);

    for lat_bucket in (center_lat_bucket - radius)..=(center_lat_bucket + radius) {
        for lon_bucket in (center_lon_bucket - radius)..=(center_lon_bucket + radius) {
            let tile = TileKey {
                zoom_bucket: spec.zoom_bucket,
                lat_bucket,
                lon_bucket,
            };
            if db::tile_exists(&connection, tile).ok()? {
                ready_assets += 1;
            } else if builders::is_pending(tile) {
                pending_assets += 1;
            }
        }
    }

    Some(FocusContourRegionStatus {
        ready_assets,
        pending_assets,
        total_assets,
    })
}

/// Returns the path to `global_land_overview.gpkg` once it's ready.
/// On first call with no pre-existing file, spawns a one-time background
/// build (gdalwarp over all available SRTM tiles → gdal_contour at 500 m).
/// Returns `None` while building; the caller should try again next frame.
pub fn ensure_global_land_overview(selected_root: Option<&Path>) -> Option<PathBuf> {
    let srtm_root = terrain_assets::find_srtm_root(selected_root)?;
    let cache_root = db::focus_cache_root(selected_root)?;
    let output_path = cache_root.join("global_land_overview.gpkg");

    if output_path.exists() {
        return Some(output_path);
    }

    if gdal::global_overview_building().load(Ordering::Relaxed) {
        return None;
    }

    // Prefer a pre-existing VRT in the parent of the SRTM tile directory
    // (many distributions ship one, e.g. SRTM_GL1_srtm.vrt alongside
    // SRTM_GL1_srtm/).  Failing that, scan for tiles and build our own.
    let source_vrt = gdal::find_prebuilt_vrt(&srtm_root);
    let tiles_for_vrt = if source_vrt.is_none() {
        let t = gdal::find_all_srtm_tiles(&srtm_root);
        if t.is_empty() {
            return None;
        }
        t
    } else {
        Vec::new()
    };

    gdal::global_overview_building().store(true, Ordering::SeqCst);
    let cache_root_clone = cache_root.clone();

    std::thread::spawn(move || {
        let tmp_dir = cache_root_clone.join(TEMP_DIR_NAME);
        let tmp_tif = tmp_dir.join("global_overview.tmp.tif");
        let tmp_gpkg = tmp_dir.join("global_overview.tmp.gpkg");
        let out = cache_root_clone.join("global_land_overview.gpkg");
        let _ = gdal::build_global_overview(
            source_vrt.as_deref(),
            &tiles_for_vrt,
            &tmp_tif,
            &tmp_gpkg,
            &out,
        );
        gdal::global_overview_building().store(false, Ordering::SeqCst);
        crate::app::request_repaint();
    });

    None
}

pub fn ensure_global_coastline_cache(selected_root: Option<&Path>) -> Option<PathBuf> {
    let data_root = terrain_assets::find_data_root(selected_root)?;
    let cache_root = db::focus_cache_root(selected_root)?;
    let output_path = cache_root.join("gebco_2025_coastline_0m.gpkg");

    if output_path.exists() {
        return Some(output_path);
    }

    if gdal::global_coastline_building().load(Ordering::Relaxed) {
        return None;
    }

    let tiles = gdal::find_gebco_topography_tiles(&data_root);
    if tiles.is_empty() {
        return None;
    }

    gdal::global_coastline_building().store(true, Ordering::SeqCst);
    let cache_root_clone = cache_root.clone();
    std::thread::spawn(move || {
        let tmp_dir = cache_root_clone.join(TEMP_DIR_NAME);
        let tmp_vrt = tmp_dir.join("global_coastline.tmp.vrt");
        let tmp_gpkg = tmp_dir.join("global_coastline.tmp.gpkg");
        let out = cache_root_clone.join("gebco_2025_coastline_0m.gpkg");
        let _ = gdal::build_global_coastline(&tiles, &tmp_vrt, &tmp_gpkg, &out);
        gdal::global_coastline_building().store(false, Ordering::SeqCst);
        crate::app::request_repaint();
    });

    None
}

pub fn terminate_active_gdal_jobs() {
    gdal::shutdown_requested().store(true, Ordering::SeqCst);

    let pids = if let Ok(mut guard) = gdal::active_children().lock() {
        let pids = guard.iter().copied().collect::<Vec<_>>();
        guard.clear();
        pids
    } else {
        Vec::new()
    };

    for pid in &pids {
        let _ = Command::new("/bin/kill")
            .args(["-TERM", &pid.to_string()])
            .status();
    }

    std::thread::sleep(Duration::from_millis(150));

    for pid in &pids {
        let _ = Command::new("/bin/kill")
            .args(["-KILL", &pid.to_string()])
            .status();
    }
}

pub fn is_global_coastline_building() -> bool {
    gdal::global_coastline_building().load(Ordering::Relaxed)
}
