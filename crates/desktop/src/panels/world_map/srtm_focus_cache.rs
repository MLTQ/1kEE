use crate::model::GeoPoint;
use crate::terrain_assets;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const BUILD_TIMEOUT: Duration = Duration::from_secs(90);
const STALE_TEMP_AGE: Duration = Duration::from_secs(45);

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
struct FocusContourSpec {
    half_extent_deg: f32,
    raster_size: u32,
    interval_m: i32,
    simplify_step: usize,
    feature_budget: usize,
    zoom_bucket: i32,
}

#[derive(Clone, Copy)]
struct GeoBounds {
    min_lat: f32,
    max_lat: f32,
    min_lon: f32,
    max_lon: f32,
}

pub fn ensure_focus_contours(
    selected_root: Option<&Path>,
    focus: GeoPoint,
    zoom: f32,
) -> Option<FocusContourAsset> {
    ensure_focus_contour_region(selected_root, focus, zoom, 0)
        .into_iter()
        .next()
}

pub fn feature_budget_for_zoom(zoom: f32) -> usize {
    spec_for_zoom(zoom).feature_budget
}

pub fn half_extent_for_zoom(zoom: f32) -> f32 {
    spec_for_zoom(zoom).half_extent_deg
}

pub fn contour_interval_for_zoom(zoom: f32) -> i32 {
    spec_for_zoom(zoom).interval_m
}

pub fn bucket_radius_for_target_radius_miles(zoom: f32, radius_miles: f32) -> i32 {
    let half_extent_deg = half_extent_for_zoom(zoom);
    let half_extent_km = half_extent_deg * 111.32;
    let bucket_step_km = half_extent_deg * 0.45 * 111.32;
    let target_km = radius_miles * 1.609_34;

    if target_km <= half_extent_km {
        0
    } else {
        (((target_km - half_extent_km) / bucket_step_km).ceil() as i32).clamp(0, 8)
    }
}

pub fn ensure_focus_contour_region(
    selected_root: Option<&Path>,
    focus: GeoPoint,
    zoom: f32,
    radius: i32,
) -> Vec<FocusContourAsset> {
    let Some(srtm_root) = terrain_assets::find_srtm_root(selected_root) else {
        return Vec::new();
    };
    let Some(cache_root) = focus_cache_root(selected_root) else {
        return Vec::new();
    };
    let spec = spec_for_zoom(zoom);
    let bucket_step = spec.half_extent_deg * 0.45;
    let center_lat_bucket = (focus.lat / bucket_step).round() as i32;
    let center_lon_bucket = (focus.lon / bucket_step).round() as i32;
    let mut assets = Vec::new();

    for lat_bucket in (center_lat_bucket - radius)..=(center_lat_bucket + radius) {
        for lon_bucket in (center_lon_bucket - radius)..=(center_lon_bucket + radius) {
            if let Some(asset) = ensure_bucket_asset(
                &srtm_root,
                &cache_root,
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

pub fn focus_contour_region_status(
    selected_root: Option<&Path>,
    focus: GeoPoint,
    zoom: f32,
    radius: i32,
) -> Option<FocusContourRegionStatus> {
    let _ = terrain_assets::find_srtm_root(selected_root)?;
    let cache_root = focus_cache_root(selected_root)?;
    let spec = spec_for_zoom(zoom);
    let bucket_step = spec.half_extent_deg * 0.45;
    let center_lat_bucket = (focus.lat / bucket_step).round() as i32;
    let center_lon_bucket = (focus.lon / bucket_step).round() as i32;
    let mut ready_assets = 0usize;
    let mut pending_assets = 0usize;
    let total_assets = ((radius * 2 + 1) as usize).pow(2);

    for lat_bucket in (center_lat_bucket - radius)..=(center_lat_bucket + radius) {
        for lon_bucket in (center_lon_bucket - radius)..=(center_lon_bucket + radius) {
            let stem = format!("z{}_lat{}_lon{}", spec.zoom_bucket, lat_bucket, lon_bucket);
            let gpkg_path = cache_root.join(format!("{stem}.gpkg"));
            if gpkg_path.exists() {
                ready_assets += 1;
            } else if is_pending(&gpkg_path) {
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

pub fn terminate_active_gdal_jobs() {
    shutdown_requested().store(true, Ordering::SeqCst);

    let pids = if let Ok(mut guard) = active_children().lock() {
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

fn spec_for_zoom(zoom: f32) -> FocusContourSpec {
    if zoom < 1.0 {
        FocusContourSpec {
            half_extent_deg: 3.6,
            raster_size: 384,
            interval_m: 50,
            simplify_step: 5,
            feature_budget: 320,
            zoom_bucket: 0,
        }
    } else if zoom < 2.0 {
        FocusContourSpec {
            half_extent_deg: 2.2,
            raster_size: 512,
            interval_m: 25,
            simplify_step: 4,
            feature_budget: 360,
            zoom_bucket: 1,
        }
    } else if zoom < 3.0 {
        FocusContourSpec {
            half_extent_deg: 1.4,
            raster_size: 576,
            interval_m: 20,
            simplify_step: 4,
            feature_budget: 400,
            zoom_bucket: 2,
        }
    } else if zoom < 4.5 {
        FocusContourSpec {
            half_extent_deg: 0.9,
            raster_size: 640,
            interval_m: 10,
            simplify_step: 3,
            feature_budget: 440,
            zoom_bucket: 3,
        }
    } else if zoom < 6.5 {
        FocusContourSpec {
            half_extent_deg: 0.55,
            raster_size: 704,
            interval_m: 10,
            simplify_step: 3,
            feature_budget: 480,
            zoom_bucket: 4,
        }
    } else if zoom < 9.5 {
        FocusContourSpec {
            half_extent_deg: 0.3,
            raster_size: 768,
            interval_m: 5,
            simplify_step: 2,
            feature_budget: 560,
            zoom_bucket: 5,
        }
    } else {
        FocusContourSpec {
            half_extent_deg: 0.16,
            raster_size: 896,
            interval_m: 5,
            simplify_step: 2,
            feature_budget: 640,
            zoom_bucket: 6,
        }
    }
}

impl GeoBounds {
    fn around(focus: GeoPoint, half_extent_deg: f32) -> Self {
        Self {
            min_lat: (focus.lat - half_extent_deg).clamp(-89.999, 89.999),
            max_lat: (focus.lat + half_extent_deg).clamp(-89.999, 89.999),
            min_lon: focus.lon - half_extent_deg,
            max_lon: focus.lon + half_extent_deg,
        }
    }
}

fn ensure_bucket_asset(
    srtm_root: &Path,
    cache_root: &Path,
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
    let stem = format!("z{}_lat{}_lon{}", spec.zoom_bucket, lat_bucket, lon_bucket);
    let gpkg_path = cache_root.join(format!("{stem}.gpkg"));
    let tif_path = cache_root.join(format!("{stem}.tif"));
    cleanup_stale_bucket_artifacts(&gpkg_path, &tif_path);

    if gpkg_path.exists() {
        return Some(FocusContourAsset {
            path: gpkg_path,
            simplify_step: spec.simplify_step,
            zoom_bucket: spec.zoom_bucket,
            lat_bucket,
            lon_bucket,
        });
    }

    if is_pending(&gpkg_path) {
        return None;
    }

    let pending = pending_set();
    let mut guard = pending.lock().ok()?;
    if !guard.insert(gpkg_path.clone()) {
        return None;
    }
    drop(guard);

    let srtm_root = srtm_root.to_path_buf();
    let tif_path_for_thread = tif_path.clone();
    let gpkg_path_for_thread = gpkg_path.clone();
    std::thread::spawn(move || {
        let _ = build_focus_contours(
            &srtm_root,
            &tif_path_for_thread,
            &gpkg_path_for_thread,
            bounds,
            spec,
        );
        if let Ok(mut guard) = pending_set().lock() {
            guard.remove(&gpkg_path_for_thread);
        }
    });

    None
}

fn focus_cache_root(selected_root: Option<&Path>) -> Option<PathBuf> {
    let root = terrain_assets::find_derived_root(selected_root)
        .unwrap_or_else(|| std::env::temp_dir().join("1kee-derived"));
    let cache_root = root.join("terrain/srtm_focus_cache");
    fs::create_dir_all(&cache_root).ok()?;
    Some(cache_root)
}

fn build_focus_contours(
    srtm_root: &Path,
    tif_path: &Path,
    gpkg_path: &Path,
    bounds: GeoBounds,
    spec: FocusContourSpec,
) -> Option<()> {
    if shutdown_requested().load(Ordering::Relaxed) {
        return None;
    }

    cleanup_transient_artifacts(tif_path, gpkg_path);
    let tiles = tile_paths_for_bounds(srtm_root, bounds);
    if tiles.is_empty() {
        return None;
    }

    if !tif_path.exists() {
        if let Some(parent) = tif_path.parent() {
            fs::create_dir_all(parent).ok()?;
        }
        let tmp_tif_path = tif_path.with_extension("tmp.tif");
        let _ = fs::remove_file(&tmp_tif_path);
        run_gdalwarp(&tiles, &tmp_tif_path, bounds, spec).ok()?;
        fs::rename(&tmp_tif_path, tif_path).ok()?;
    }

    if shutdown_requested().load(Ordering::Relaxed) {
        return None;
    }

    if gpkg_path.exists() {
        fs::remove_file(gpkg_path).ok();
    }
    let tmp_gpkg_path = gpkg_path.with_extension("tmp.gpkg");
    let _ = fs::remove_file(&tmp_gpkg_path);
    run_gdal_contour(tif_path, &tmp_gpkg_path, spec.interval_m).ok()?;
    fs::rename(&tmp_gpkg_path, gpkg_path).ok()?;
    Some(())
}

fn tile_paths_for_bounds(root: &Path, bounds: GeoBounds) -> Vec<PathBuf> {
    let mut tiles = Vec::new();
    let lat_start = bounds.min_lat.floor() as i32;
    let lat_end = bounds.max_lat.floor() as i32;
    let lon_start = bounds.min_lon.floor() as i32;
    let lon_end = bounds.max_lon.floor() as i32;

    for lat in lat_start..=lat_end {
        for lon in lon_start..=lon_end {
            let path = root.join(tile_name(lat, lon));
            if path.exists() {
                tiles.push(path);
            }
        }
    }

    tiles
}

fn tile_name(lat: i32, lon: i32) -> String {
    let lat_prefix = if lat >= 0 { 'N' } else { 'S' };
    let lon_prefix = if lon >= 0 { 'E' } else { 'W' };
    format!(
        "{}{:02}{}{:03}.tif",
        lat_prefix,
        lat.unsigned_abs(),
        lon_prefix,
        lon.unsigned_abs()
    )
}

fn run_gdalwarp(
    tiles: &[PathBuf],
    output_path: &Path,
    bounds: GeoBounds,
    spec: FocusContourSpec,
) -> std::io::Result<()> {
    let mut command = Command::new(gdal_tool_path("gdalwarp"));
    command.args([
        "-q",
        "-overwrite",
        "-r",
        "bilinear",
        "-dstnodata",
        "-32768",
        "-te",
        &format!("{:.6}", bounds.min_lon),
        &format!("{:.6}", bounds.min_lat),
        &format!("{:.6}", bounds.max_lon),
        &format!("{:.6}", bounds.max_lat),
        "-ts",
        &spec.raster_size.to_string(),
        &spec.raster_size.to_string(),
    ]);
    for tile in tiles {
        command.arg(tile);
    }
    command.arg(output_path);
    run_command(command, "gdalwarp")
}

fn run_gdal_contour(input_path: &Path, output_path: &Path, interval_m: i32) -> std::io::Result<()> {
    let mut command = Command::new(gdal_tool_path("gdal_contour"));
    command.args([
        "-q",
        "-f",
        "GPKG",
        "-a",
        "elevation_m",
        "-i",
        &interval_m.to_string(),
        "-nln",
        "contour",
    ]);
    command.arg(input_path);
    command.arg(output_path);
    run_command(command, "gdal_contour")
}

fn gdal_tool_path(tool: &str) -> PathBuf {
    let postgres_app = PathBuf::from(format!(
        "/Applications/Postgres.app/Contents/Versions/latest/bin/{tool}"
    ));
    if postgres_app.exists() {
        return postgres_app;
    }

    let homebrew = PathBuf::from(format!("/opt/homebrew/bin/{tool}"));
    if homebrew.exists() {
        return homebrew;
    }

    PathBuf::from(tool)
}

fn run_command(mut command: Command, label: &str) -> std::io::Result<()> {
    if shutdown_requested().load(Ordering::Relaxed) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Interrupted,
            format!("{label} cancelled during shutdown"),
        ));
    }

    let mut child = command.spawn()?;
    let pid = child.id();
    if let Ok(mut guard) = active_children().lock() {
        guard.insert(pid);
    }
    let started = Instant::now();

    loop {
        if let Some(status) = child.try_wait()? {
            if let Ok(mut guard) = active_children().lock() {
                guard.remove(&pid);
            }
            return if status.success() {
                Ok(())
            } else {
                Err(std::io::Error::other(format!(
                    "{label} failed with status {status}"
                )))
            };
        }

        if shutdown_requested().load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            if let Ok(mut guard) = active_children().lock() {
                guard.remove(&pid);
            }
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                format!("{label} cancelled during shutdown"),
            ));
        }

        if started.elapsed() >= BUILD_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            if let Ok(mut guard) = active_children().lock() {
                guard.remove(&pid);
            }
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("{label} timed out after {:?}", BUILD_TIMEOUT),
            ));
        }

        std::thread::sleep(Duration::from_millis(150));
    }
}

fn cleanup_stale_bucket_artifacts(gpkg_path: &Path, tif_path: &Path) {
    let tmp_gpkg_path = gpkg_path.with_extension("tmp.gpkg");
    let tmp_tif_path = tif_path.with_extension("tmp.tif");
    let journal_path = journal_path_for(&tmp_gpkg_path);

    if is_stale(&tmp_gpkg_path) || is_stale(&journal_path) {
        cleanup_transient_artifacts(tif_path, gpkg_path);
    }

    if !gpkg_path.exists() && is_stale(&tmp_tif_path) {
        let _ = fs::remove_file(&tmp_tif_path);
    }
}

fn cleanup_transient_artifacts(tif_path: &Path, gpkg_path: &Path) {
    let tmp_tif_path = tif_path.with_extension("tmp.tif");
    let tmp_gpkg_path = gpkg_path.with_extension("tmp.gpkg");
    let journal_path = journal_path_for(&tmp_gpkg_path);
    let wal_path = wal_path_for(&tmp_gpkg_path);
    let shm_path = shm_path_for(&tmp_gpkg_path);

    let _ = fs::remove_file(&tmp_tif_path);
    let _ = fs::remove_file(&tmp_gpkg_path);
    let _ = fs::remove_file(&journal_path);
    let _ = fs::remove_file(&wal_path);
    let _ = fs::remove_file(&shm_path);
}

fn is_stale(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    let Ok(modified) = metadata.modified() else {
        return false;
    };
    modified
        .elapsed()
        .is_ok_and(|elapsed| elapsed >= STALE_TEMP_AGE)
}

fn journal_path_for(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .map(|name| format!("{}-journal", name.to_string_lossy()))
        .unwrap_or_else(|| "cache.tmp.gpkg-journal".to_string());
    path.with_file_name(file_name)
}

fn wal_path_for(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .map(|name| format!("{}-wal", name.to_string_lossy()))
        .unwrap_or_else(|| "cache.tmp.gpkg-wal".to_string());
    path.with_file_name(file_name)
}

fn shm_path_for(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .map(|name| format!("{}-shm", name.to_string_lossy()))
        .unwrap_or_else(|| "cache.tmp.gpkg-shm".to_string());
    path.with_file_name(file_name)
}

fn pending_set() -> &'static Mutex<HashSet<PathBuf>> {
    static PENDING: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    PENDING.get_or_init(|| Mutex::new(HashSet::new()))
}

fn active_children() -> &'static Mutex<HashSet<u32>> {
    static ACTIVE: OnceLock<Mutex<HashSet<u32>>> = OnceLock::new();
    ACTIVE.get_or_init(|| Mutex::new(HashSet::new()))
}

fn shutdown_requested() -> &'static AtomicBool {
    static SHUTDOWN: OnceLock<AtomicBool> = OnceLock::new();
    SHUTDOWN.get_or_init(|| AtomicBool::new(false))
}

fn is_pending(path: &Path) -> bool {
    pending_set()
        .lock()
        .map(|guard| guard.contains(path))
        .unwrap_or(false)
}
