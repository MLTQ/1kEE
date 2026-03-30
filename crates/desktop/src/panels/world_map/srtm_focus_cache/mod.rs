use crate::model::GeoPoint;
use crate::terrain_assets::{self};
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
const LUNAR_CACHE_DB_NAME: &str = "lunar_focus_cache.sqlite";
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
    // SRTM root is only needed to spawn on-demand GDAL builds for uncached tiles.
    // Pre-built tiles in the SQLite cache are returned even without SRTM access.
    let srtm_root = terrain_assets::find_srtm_root(selected_root);
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

/// Ensure the two GEBCO-derived runtime assets exist in the cache/terrain directory:
///   - `gebco_depth_1440x720.bil`      (globe depth-fill texture)
///   - `gebco_2025_contours_200m.gpkg` (bathymetry isobaths)
///
/// Returns `(Option<depth_bil_path>, Option<contours_gpkg_path>)`.
/// Any file that already exists is returned immediately; missing ones are
/// built in a background thread and `None` is returned until complete.
/// Callers should call every frame — the function is cheap when already built.
pub fn ensure_gebco_derived(
    selected_root: Option<&Path>,
) -> (Option<PathBuf>, Option<PathBuf>) {
    let Some(data_root) = terrain_assets::find_data_root(selected_root) else {
        return (None, None);
    };
    let Some(cache_root) = db::focus_cache_root(selected_root) else {
        return (None, None);
    };

    let depth_bil = cache_root.join("gebco_depth_1440x720.bil");
    let contours_gpkg = cache_root.join("gebco_2025_contours_200m.gpkg");

    let bil_ready = depth_bil.exists();
    let gpkg_ready = contours_gpkg.exists();

    if bil_ready && gpkg_ready {
        return (Some(depth_bil), Some(contours_gpkg));
    }

    if gdal::gebco_derived_building().load(Ordering::Relaxed) {
        return (
            bil_ready.then_some(depth_bil),
            gpkg_ready.then_some(contours_gpkg),
        );
    }

    let tiles = gdal::find_gebco_topography_tiles(&data_root);
    if tiles.is_empty() {
        return (
            bil_ready.then_some(depth_bil),
            gpkg_ready.then_some(contours_gpkg),
        );
    }

    gdal::gebco_derived_building().store(true, Ordering::SeqCst);
    let cache_root_clone = cache_root.clone();
    std::thread::spawn(move || {
        let _ = gdal::build_gebco_derived(&tiles, &cache_root_clone);
        gdal::gebco_derived_building().store(false, Ordering::SeqCst);
        crate::app::request_repaint();
    });

    (
        bil_ready.then_some(depth_bil),
        gpkg_ready.then_some(contours_gpkg),
    )
}

pub fn is_gebco_derived_building() -> bool {
    gdal::gebco_derived_building().load(Ordering::Relaxed)
}

pub fn is_lunar_preview_building() -> bool {
    gdal::lunar_preview_building().load(Ordering::Relaxed)
}

/// Ensure the SLDEM2015 lunar terrain preview PNG exists in the cache/terrain
/// directory.  If the JP2 source file is found and the preview does not yet
/// exist, triggers a background GDAL conversion.
///
/// Call this when Moon Mode is active; it is cheap when already built.
pub fn ensure_lunar_preview(selected_root: Option<&Path>) {
    let out_png = match db::focus_cache_root(selected_root) {
        Some(root) => root.join("sldem2015_preview_4096.png"),
        None => return,
    };

    if out_png.exists() {
        return;
    }

    if gdal::lunar_preview_building().load(Ordering::Relaxed) {
        return;
    }

    let jp2 = match terrain_assets::find_sldem_jp2(selected_root) {
        Some(p) => p,
        None => return,
    };

    let cache_root = match db::focus_cache_root(selected_root) {
        Some(root) => root,
        None => return,
    };

    gdal::lunar_preview_building().store(true, Ordering::SeqCst);
    std::thread::spawn(move || {
        let _ = gdal::build_lunar_preview(&jp2, &cache_root);
        gdal::lunar_preview_building().store(false, Ordering::SeqCst);
        crate::app::request_repaint();
    });
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

pub fn lunar_cache_db_path(selected_root: Option<&Path>) -> Option<PathBuf> {
    Some(db::focus_cache_root(selected_root)?.join(LUNAR_CACHE_DB_NAME))
}

/// Returns `true` while any lunar contour tile build threads are running.
pub fn is_lunar_contour_building() -> bool {
    builders::lunar_pending_set()
        .lock()
        .map(|g| !g.is_empty())
        .unwrap_or(false)
}

/// Ensure lunar (SLDEM2015) contour tiles exist for the region around `focus`
/// at the current zoom level.  Mirrors `ensure_focus_contour_region` but
/// sources from a single JP2 file via `gdal_translate -projwin` instead of
/// mosaicking SRTM tiles.  Coverage is clipped to ±60° latitude.
pub fn ensure_lunar_contour_region(
    selected_root: Option<&Path>,
    focus: GeoPoint,
    zoom: f32,
) -> Vec<FocusContourAsset> {
    let Some(jp2_path) = crate::terrain_assets::find_sldem_jp2(selected_root) else {
        return Vec::new();
    };
    let Some(cache_root) = db::focus_cache_root(selected_root) else {
        return Vec::new();
    };
    let Some(cache_db_path) = lunar_cache_db_path(selected_root) else {
        return Vec::new();
    };
    let Ok(connection) = db::open_cache_db(&cache_db_path) else {
        return Vec::new();
    };

    let spec = zoom::lunar_spec_for_zoom(zoom);
    let bucket_step = spec.half_extent_deg * 0.45;
    let center_lat_bucket = (focus.lat / bucket_step).round() as i32;
    let center_lon_bucket = (focus.lon / bucket_step).round() as i32;
    let mut assets = Vec::new();

    const RADIUS: i32 = 2;
    for lat_bucket in (center_lat_bucket - RADIUS)..=(center_lat_bucket + RADIUS) {
        // Skip tiles whose centre falls outside SLDEM2015 coverage (±60° lat).
        let bucket_lat = lat_bucket as f32 * bucket_step;
        if bucket_lat.abs() > 60.0 + spec.half_extent_deg {
            continue;
        }
        for lon_bucket in (center_lon_bucket - RADIUS)..=(center_lon_bucket + RADIUS) {
            if let Some(asset) = builders::ensure_lunar_bucket_asset(
                &jp2_path,
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
