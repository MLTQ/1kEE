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
pub mod zoom; // pub so ui_overlays can access lunar_spec_for_zoom

pub use zoom::{
    bucket_radius_for_target_radius_miles, contour_interval_for_zoom, feature_budget_for_zoom,
    half_extent_for_zoom, zoom_bucket_for_zoom,
};

/// Half-extent in degrees for the lunar zoom spec at a given zoom level.
/// Used by the contour merge step to partition tiles without overlap.
pub fn lunar_half_extent_for_zoom(zoom: f32) -> f32 {
    zoom::lunar_spec_for_zoom(zoom).half_extent_deg
}

/// Returns the set of `(lat_bucket, lon_bucket)` pairs that are already built
/// in the *lunar* cache DB for the given zoom level.  Parallel to
/// `ready_tile_buckets` but queries `lunar_focus_cache.sqlite`.
pub fn ready_lunar_tile_buckets(
    selected_root: Option<&Path>,
    focus: GeoPoint,
    zoom: f32,
    radius: i32,
) -> HashSet<(i32, i32)> {
    let mut set = HashSet::new();
    let Some(cache_db_path) = lunar_cache_db_path(selected_root) else {
        return set;
    };
    let Ok(connection) = db::open_cache_db_read_only(&cache_db_path) else {
        return set;
    };
    let spec = zoom::lunar_spec_for_zoom(zoom);
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

pub fn mars_half_extent_for_zoom(zoom: f32) -> f32 {
    zoom::mars_spec_for_zoom(zoom).half_extent_deg
}

pub fn ready_mars_tile_buckets(
    selected_root: Option<&Path>,
    focus: GeoPoint,
    zoom: f32,
    radius: i32,
) -> HashSet<(i32, i32)> {
    let mut set = HashSet::new();
    let Some(cache_db_path) = mars_cache_db_path(selected_root) else {
        return set;
    };
    let Ok(connection) = db::open_cache_db_read_only(&cache_db_path) else {
        return set;
    };
    let spec = zoom::mars_spec_for_zoom(zoom);
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

const BUILD_TIMEOUT: Duration = Duration::from_secs(90);
const CACHE_DB_NAME: &str = "srtm_focus_cache.sqlite";
const LUNAR_CACHE_DB_NAME: &str = "lunar_focus_cache.sqlite";
const TEMP_DIR_NAME: &str = "srtm_focus_tmp";
/// Bound externally supplied region requests before turning them into a tile
/// rectangle. Local terrain currently needs six rings; this cap keeps a bad
/// caller from allocating an unbounded manifest/query window on the UI thread.
const MAX_CONTOUR_REGION_RADIUS: i32 = 16;

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

/// The ready tiles and progress numbers for one local visible/build window.
/// It is derived from the loader's manifest snapshot, so overlays do not need
/// to issue another round of SQLite lookups after the region is selected.
#[derive(Clone)]
pub struct LocalContourRegionState {
    pub ready_buckets: HashSet<(i32, i32)>,
    pub status: FocusContourRegionStatus,
}

#[derive(Clone, Copy)]
pub struct FocusContourSpec {
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

fn bounded_region_radius(radius: i32) -> i32 {
    radius.clamp(0, MAX_CONTOUR_REGION_RADIUS)
}

fn region_radii(prefetch_radius: i32, build_radius: i32) -> (i32, i32) {
    let prefetch_radius = bounded_region_radius(prefetch_radius);
    (prefetch_radius, build_radius.clamp(0, prefetch_radius))
}

fn tile_ring_distance(
    lat_bucket: i32,
    lon_bucket: i32,
    center_lat_bucket: i32,
    center_lon_bucket: i32,
) -> i64 {
    (i64::from(lat_bucket) - i64::from(center_lat_bucket))
        .abs()
        .max((i64::from(lon_bucket) - i64::from(center_lon_bucket)).abs())
}

fn bucket_has_latitude_coverage(
    lat_bucket: i32,
    bucket_step: f32,
    spec: FocusContourSpec,
    max_abs_lat: Option<f32>,
) -> bool {
    max_abs_lat.map_or(true, |max_abs_lat| {
        (lat_bucket as f32 * bucket_step).abs() <= max_abs_lat + spec.half_extent_deg
    })
}

/// Visit the center tile and nearby rings before outer prefetch rings. This
/// lets the bounded builder slots serve the visible local scene first.
fn ordered_tile_buckets(
    center_lat_bucket: i32,
    center_lon_bucket: i32,
    radius: i32,
) -> Vec<(i32, i32)> {
    let radius = bounded_region_radius(radius);
    let side = (radius * 2 + 1) as usize;
    let mut buckets = Vec::with_capacity(side * side);
    for lat_bucket in center_lat_bucket.saturating_sub(radius)..=center_lat_bucket.saturating_add(radius) {
        for lon_bucket in center_lon_bucket.saturating_sub(radius)..=center_lon_bucket.saturating_add(radius) {
            let lat_distance = (i64::from(lat_bucket) - i64::from(center_lat_bucket)).abs();
            let lon_distance = (i64::from(lon_bucket) - i64::from(center_lon_bucket)).abs();
            buckets.push((lat_bucket, lon_bucket, lat_distance, lon_distance));
        }
    }
    buckets.sort_unstable_by_key(|(lat_bucket, lon_bucket, lat_distance, lon_distance)| {
        (
            (*lat_distance).max(*lon_distance),
            *lat_distance + *lon_distance,
            *lat_bucket,
            *lon_bucket,
        )
    });
    buckets
        .into_iter()
        .map(|(lat_bucket, lon_bucket, _, _)| (lat_bucket, lon_bucket))
        .collect()
}

fn local_region_state_from_assets(
    assets: &[FocusContourAsset],
    focus: GeoPoint,
    spec: FocusContourSpec,
    radius: i32,
    max_abs_lat: Option<f32>,
    pending_tiles: HashSet<TileKey>,
    sourceless_tiles: HashSet<TileKey>,
) -> LocalContourRegionState {
    let radius = bounded_region_radius(radius);
    let bucket_step = spec.half_extent_deg * 0.45;
    let center_lat_bucket = (focus.lat / bucket_step).round() as i32;
    let center_lon_bucket = (focus.lon / bucket_step).round() as i32;
    let mut ready_buckets = HashSet::new();

    for asset in assets {
        if asset.zoom_bucket == spec.zoom_bucket
            && tile_ring_distance(
                asset.lat_bucket,
                asset.lon_bucket,
                center_lat_bucket,
                center_lon_bucket,
            ) <= i64::from(radius)
            && bucket_has_latitude_coverage(asset.lat_bucket, bucket_step, spec, max_abs_lat)
        {
            ready_buckets.insert((asset.lat_bucket, asset.lon_bucket));
        }
    }

    // Progress counts only buckets that can ever hold contours. Sourceless
    // buckets join `ready_buckets` afterwards purely to suppress their pulse.
    let ready_assets = ready_buckets.len();
    let mut total_assets = 0usize;
    let mut pending_assets = 0usize;
    for (lat_bucket, lon_bucket) in
        ordered_tile_buckets(center_lat_bucket, center_lon_bucket, radius)
    {
        if !bucket_has_latitude_coverage(lat_bucket, bucket_step, spec, max_abs_lat) {
            continue;
        }
        let tile = TileKey {
            zoom_bucket: spec.zoom_bucket,
            lat_bucket,
            lon_bucket,
        };
        if !ready_buckets.contains(&(lat_bucket, lon_bucket)) && sourceless_tiles.contains(&tile) {
            // Open ocean: no source terrain exists here, so the bucket is
            // resolved rather than loading. It is neither an asset the region
            // is waiting on nor a tile the pulse grid should keep animating.
            ready_buckets.insert((lat_bucket, lon_bucket));
            continue;
        }
        total_assets += 1;
        if !ready_buckets.contains(&(lat_bucket, lon_bucket)) && pending_tiles.contains(&tile) {
            pending_assets += 1;
        }
    }

    LocalContourRegionState {
        status: FocusContourRegionStatus {
            ready_assets,
            pending_assets,
            total_assets,
        },
        ready_buckets,
    }
}

/// Build one local-progress snapshot from the ready assets the loader already
/// selected. This avoids duplicate manifest scans for the progress card and
/// pulse grid during camera motion.
pub fn local_contour_region_state(
    active_body: crate::model::ActiveBody,
    assets: &[FocusContourAsset],
    focus: GeoPoint,
    zoom: f32,
    radius: i32,
) -> LocalContourRegionState {
    match active_body {
        crate::model::ActiveBody::Earth => local_region_state_from_assets(
            assets,
            focus,
            zoom::spec_for_zoom(zoom),
            radius,
            None,
            builders::pending_set()
                .lock()
                .map(|guard| guard.clone())
                .unwrap_or_default(),
            builders::sourceless_tile_set(),
        ),
        // The lunar and Mars sources are global rasters, so every in-latitude
        // bucket has source coverage; only SRTM has ocean gaps.
        crate::model::ActiveBody::Moon => local_region_state_from_assets(
            assets,
            focus,
            zoom::lunar_spec_for_zoom(zoom),
            radius,
            Some(60.0),
            builders::lunar_pending_set()
                .lock()
                .map(|guard| guard.clone())
                .unwrap_or_default(),
            HashSet::new(),
        ),
        crate::model::ActiveBody::Mars => local_region_state_from_assets(
            assets,
            focus,
            zoom::mars_spec_for_zoom(zoom),
            radius,
            Some(80.0),
            builders::mars_pending_set()
                .lock()
                .map(|guard| guard.clone())
                .unwrap_or_default(),
            HashSet::new(),
        ),
    }
}

/// Current in-process contour-cache write revision. Local region snapshots
/// compare this before reusing their manifest result.
pub fn contour_manifest_revision() -> u64 {
    builders::manifest_revision()
}

/// Release failed-build cooldowns after an explicit in-memory cache reset.
pub fn clear_contour_build_backoffs() {
    builders::clear_failed_build_backoffs();
}

/// Renderer-facing region selection should never need to mutate an established
/// cache. A missing database is the one exception: retain the first-run path
/// that initializes its schema so on-demand builders can populate it.
fn open_region_cache_db(path: &Path) -> rusqlite::Result<Connection> {
    match db::open_cache_db_read_only(path) {
        Ok(connection) => Ok(connection),
        Err(_) if !path.exists() => db::open_cache_db(path),
        Err(error) => Err(error),
    }
}

pub fn ensure_focus_contour_region(
    selected_root: Option<&Path>,
    focus: GeoPoint,
    zoom: f32,
    prefetch_radius: i32,
    build_radius: i32,
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
    // Open one read-only connection for all tile checks. A missing cache is
    // initialized only for first-run builder scheduling; existing caches stay
    // readable even if their volume cannot accept WAL/schema writes.
    let Ok(connection) = open_region_cache_db(&cache_db_path) else {
        return Vec::new();
    };
    let spec = zoom::spec_for_zoom(zoom);
    let bucket_step = spec.half_extent_deg * 0.45;
    let center_lat_bucket = (focus.lat / bucket_step).round() as i32;
    let center_lon_bucket = (focus.lon / bucket_step).round() as i32;
    let (prefetch_radius, build_radius) = region_radii(prefetch_radius, build_radius);
    let min_lat_bucket = center_lat_bucket.saturating_sub(prefetch_radius);
    let max_lat_bucket = center_lat_bucket.saturating_add(prefetch_radius);
    let min_lon_bucket = center_lon_bucket.saturating_sub(prefetch_radius);
    let max_lon_bucket = center_lon_bucket.saturating_add(prefetch_radius);
    let Ok(manifest) = db::contour_manifest_window(
        &connection,
        spec.zoom_bucket,
        min_lat_bucket,
        max_lat_bucket,
        min_lon_bucket,
        max_lon_bucket,
    ) else {
        return Vec::new();
    };
    let mut assets = Vec::new();

    for (lat_bucket, lon_bucket) in
        ordered_tile_buckets(center_lat_bucket, center_lon_bucket, prefetch_radius)
    {
        let allow_build = tile_ring_distance(
            lat_bucket,
            lon_bucket,
            center_lat_bucket,
            center_lon_bucket,
        ) <= i64::from(build_radius);
        if let Some(asset) = builders::ensure_bucket_asset(
            srtm_root.as_deref(),
            &cache_root,
            &cache_db_path,
            manifest.get(&(lat_bucket, lon_bucket)).copied(),
            allow_build,
            spec,
            lat_bucket,
            lon_bucket,
            bucket_step,
        ) {
            assets.push(asset);
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
    let Ok(connection) = db::open_cache_db_read_only(&cache_db_path) else {
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
    let connection = db::open_cache_db_read_only(&cache_db_path).ok()?;
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
pub fn ensure_gebco_derived(selected_root: Option<&Path>) -> (Option<PathBuf>, Option<PathBuf>) {
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

pub fn mars_cache_db_path(selected_root: Option<&Path>) -> Option<PathBuf> {
    Some(db::focus_cache_root(selected_root)?.join("mars_ctx_cache.sqlite"))
}

/// Returns `true` while any lunar contour tile build threads are running.
pub fn is_lunar_contour_building() -> bool {
    builders::lunar_pending_set()
        .lock()
        .map(|g| !g.is_empty())
        .unwrap_or(false)
}

pub fn is_mars_contour_building() -> bool {
    builders::mars_pending_set()
        .lock()
        .map(|g| !g.is_empty())
        .unwrap_or(false)
}

/// (ready, building, total) for the lunar tile grid around `focus`.
/// Used by the progress overlay in lunar local terrain mode.
pub fn lunar_tile_counts(
    selected_root: Option<&Path>,
    focus: GeoPoint,
    zoom: f32,
    radius: i32,
) -> (usize, usize, usize) {
    let Some(cache_db_path) = lunar_cache_db_path(selected_root) else {
        return (0, 0, 0);
    };
    let Ok(connection) = db::open_cache_db_read_only(&cache_db_path) else {
        return (0, 0, 0);
    };
    let spec = zoom::lunar_spec_for_zoom(zoom);
    let bucket_step = spec.half_extent_deg * 0.45;
    let center_lat_bucket = (focus.lat / bucket_step).round() as i32;
    let center_lon_bucket = (focus.lon / bucket_step).round() as i32;

    let building = builders::lunar_pending_set()
        .lock()
        .map(|g| g.len())
        .unwrap_or(0);

    let mut ready = 0usize;
    let mut total = 0usize;
    for lat_bucket in (center_lat_bucket - radius)..=(center_lat_bucket + radius) {
        let bucket_lat = lat_bucket as f32 * bucket_step;
        if bucket_lat.abs() > 60.0 + spec.half_extent_deg {
            continue; // outside SLDEM coverage
        }
        for lon_bucket in (center_lon_bucket - radius)..=(center_lon_bucket + radius) {
            total += 1;
            let tile = TileKey {
                zoom_bucket: spec.zoom_bucket,
                lat_bucket,
                lon_bucket,
            };
            if db::tile_exists(&connection, tile).unwrap_or(false) {
                ready += 1;
            }
        }
    }
    (ready, building, total)
}

pub fn mars_tile_counts(
    selected_root: Option<&Path>,
    focus: GeoPoint,
    zoom: f32,
    radius: i32,
) -> (usize, usize, usize) {
    let Some(cache_db_path) = mars_cache_db_path(selected_root) else {
        return (0, 0, 0);
    };
    let Ok(connection) = db::open_cache_db_read_only(&cache_db_path) else {
        return (0, 0, 0);
    };
    let spec = zoom::mars_spec_for_zoom(zoom);
    let bucket_step = spec.half_extent_deg * 0.45;
    let center_lat_bucket = (focus.lat / bucket_step).round() as i32;
    let center_lon_bucket = (focus.lon / bucket_step).round() as i32;

    let building = builders::mars_pending_set()
        .lock()
        .map(|g| g.len())
        .unwrap_or(0);

    let mut ready = 0usize;
    let mut total = 0usize;
    for lat_bucket in (center_lat_bucket - radius)..=(center_lat_bucket + radius) {
        let bucket_lat = lat_bucket as f32 * bucket_step;
        if bucket_lat.abs() > 80.0 + spec.half_extent_deg {
            continue; // MRO CTX doesn't have good polar coverage
        }
        for lon_bucket in (center_lon_bucket - radius)..=(center_lon_bucket + radius) {
            total += 1;
            let tile = TileKey {
                zoom_bucket: spec.zoom_bucket,
                lat_bucket,
                lon_bucket,
            };
            if db::tile_exists(&connection, tile).unwrap_or(false) {
                ready += 1;
            }
        }
    }
    (ready, building, total)
}

/// Ensure lunar (SLDEM2015) contour tiles exist for the region around `focus`
/// at the current zoom level.  Mirrors `ensure_focus_contour_region` but
/// sources from a single JP2 file via `gdal_translate -projwin` instead of
/// mosaicking SRTM tiles.  Coverage is clipped to ±60° latitude.
pub fn ensure_lunar_contour_region(
    selected_root: Option<&Path>,
    focus: GeoPoint,
    zoom: f32,
    prefetch_radius: i32,
    build_radius: i32,
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
    let Ok(connection) = open_region_cache_db(&cache_db_path) else {
        return Vec::new();
    };

    let spec = zoom::lunar_spec_for_zoom(zoom);
    let bucket_step = spec.half_extent_deg * 0.45;
    let center_lat_bucket = (focus.lat / bucket_step).round() as i32;
    let center_lon_bucket = (focus.lon / bucket_step).round() as i32;
    let (prefetch_radius, build_radius) = region_radii(prefetch_radius, build_radius);
    let Ok(manifest) = db::contour_manifest_window(
        &connection,
        spec.zoom_bucket,
        center_lat_bucket.saturating_sub(prefetch_radius),
        center_lat_bucket.saturating_add(prefetch_radius),
        center_lon_bucket.saturating_sub(prefetch_radius),
        center_lon_bucket.saturating_add(prefetch_radius),
    ) else {
        return Vec::new();
    };
    let mut assets = Vec::new();

    for (lat_bucket, lon_bucket) in
        ordered_tile_buckets(center_lat_bucket, center_lon_bucket, prefetch_radius)
    {
        // Skip tiles whose centre falls outside SLDEM2015 coverage (±60° lat).
        if !bucket_has_latitude_coverage(lat_bucket, bucket_step, spec, Some(60.0)) {
            continue;
        }
        let allow_build = tile_ring_distance(
            lat_bucket,
            lon_bucket,
            center_lat_bucket,
            center_lon_bucket,
        ) <= i64::from(build_radius);
        if let Some(asset) = builders::ensure_lunar_bucket_asset(
            &jp2_path,
            &cache_root,
            &cache_db_path,
            manifest.get(&(lat_bucket, lon_bucket)).copied(),
            allow_build,
            spec,
            lat_bucket,
            lon_bucket,
            bucket_step,
        ) {
            assets.push(asset);
        }
    }

    assets
}

pub fn ensure_mars_contour_region(
    selected_root: Option<&Path>,
    focus: GeoPoint,
    zoom: f32,
    prefetch_radius: i32,
    build_radius: i32,
) -> Vec<FocusContourAsset> {
    // The CTX data lives in <mars_root>/mars_data/ as 44 k per-DTM subdirectories.
    // MOLA tiles (global fallback) live in <mars_root>/MOLA/ as megt*.img files.
    let Some(data_root) = terrain_assets::find_mars_data_root(selected_root) else {
        return Vec::new();
    };
    let has_ctx = data_root.join("mars_data").is_dir();
    let mola_tiles = terrain_assets::find_mola_tiles(selected_root);
    if !has_ctx && mola_tiles.is_empty() {
        return Vec::new();
    }
    let Some(cache_root) = db::focus_cache_root(selected_root) else {
        return Vec::new();
    };
    let Some(cache_db_path) = mars_cache_db_path(selected_root) else {
        return Vec::new();
    };
    let Ok(connection) = open_region_cache_db(&cache_db_path) else {
        return Vec::new();
    };

    let spec = zoom::mars_spec_for_zoom(zoom);
    let bucket_step = spec.half_extent_deg * 0.45;
    let center_lat_bucket = (focus.lat / bucket_step).round() as i32;
    let center_lon_bucket = (focus.lon / bucket_step).round() as i32;
    let (prefetch_radius, build_radius) = region_radii(prefetch_radius, build_radius);
    let Ok(manifest) = db::contour_manifest_window(
        &connection,
        spec.zoom_bucket,
        center_lat_bucket.saturating_sub(prefetch_radius),
        center_lat_bucket.saturating_add(prefetch_radius),
        center_lon_bucket.saturating_sub(prefetch_radius),
        center_lon_bucket.saturating_add(prefetch_radius),
    ) else {
        return Vec::new();
    };
    let mut assets = Vec::new();

    for (lat_bucket, lon_bucket) in
        ordered_tile_buckets(center_lat_bucket, center_lon_bucket, prefetch_radius)
    {
        if !bucket_has_latitude_coverage(lat_bucket, bucket_step, spec, Some(80.0)) {
            continue;
        }
        let allow_build = tile_ring_distance(
            lat_bucket,
            lon_bucket,
            center_lat_bucket,
            center_lon_bucket,
        ) <= i64::from(build_radius);
        if let Some(asset) = builders::ensure_mars_bucket_asset(
            &data_root,
            &mola_tiles,
            &cache_root,
            &cache_db_path,
            manifest.get(&(lat_bucket, lon_bucket)).copied(),
            allow_build,
            spec,
            lat_bucket,
            lon_bucket,
            bucket_step,
        ) {
            assets.push(asset);
        }
    }

    assets
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordered_tile_buckets_visits_the_center_before_prefetch_rings() {
        let buckets = ordered_tile_buckets(10, -4, 2);

        assert_eq!(buckets.len(), 25);
        assert_eq!(buckets.first(), Some(&(10, -4)));
        let distances: Vec<_> = buckets
            .iter()
            .map(|&(lat_bucket, lon_bucket)| {
                tile_ring_distance(lat_bucket, lon_bucket, 10, -4)
            })
            .collect();
        assert!(distances.windows(2).all(|pair| pair[0] <= pair[1]));
        assert_eq!(distances.last(), Some(&2));
    }

    #[test]
    fn sourceless_ocean_buckets_resolve_instead_of_pulsing() {
        let zoom = 6.0;
        let spec = zoom::spec_for_zoom(zoom);
        let ready = FocusContourAsset {
            path: PathBuf::from("test-cache.sqlite"),
            simplify_step: spec.simplify_step,
            zoom_bucket: spec.zoom_bucket,
            lat_bucket: 0,
            lon_bucket: 0,
        };
        // The whole outer ring is open ocean: no SRTM file covers it.
        let sourceless: HashSet<TileKey> = (-1..=1)
            .flat_map(|lat_bucket| (-1..=1).map(move |lon_bucket| (lat_bucket, lon_bucket)))
            .filter(|&bucket| bucket != (0, 0))
            .map(|(lat_bucket, lon_bucket)| TileKey {
                zoom_bucket: spec.zoom_bucket,
                lat_bucket,
                lon_bucket,
            })
            .collect();

        let state = local_region_state_from_assets(
            &[ready],
            GeoPoint { lat: 0.0, lon: 0.0 },
            spec,
            1,
            None,
            HashSet::new(),
            sourceless,
        );

        // Every bucket in the window is resolved, so nothing keeps animating.
        assert_eq!(state.ready_buckets.len(), 9);
        // Ocean buckets are not assets the region is still waiting on.
        assert_eq!(state.status.ready_assets, 1);
        assert_eq!(state.status.total_assets, 1);
        assert_eq!(state.status.pending_assets, 0);
    }

    #[test]
    fn local_region_state_reuses_ready_assets_without_manifest_queries() {
        let zoom = 6.0;
        let spec = zoom::spec_for_zoom(zoom);
        let ready = FocusContourAsset {
            path: PathBuf::from("test-cache.sqlite"),
            simplify_step: spec.simplify_step,
            zoom_bucket: spec.zoom_bucket,
            lat_bucket: 0,
            lon_bucket: 0,
        };

        let state = local_contour_region_state(
            crate::model::ActiveBody::Earth,
            &[ready],
            GeoPoint { lat: 0.0, lon: 0.0 },
            zoom,
            1,
        );

        assert!(state.ready_buckets.contains(&(0, 0)));
        assert_eq!(state.status.ready_assets, 1);
        assert_eq!(state.status.total_assets, 9);
    }
}
