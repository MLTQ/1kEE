use super::db::{
    cleanup_temp_tile_artifacts, import_coastline_into_cache, import_tile_into_cache,
    journal_path_for, shm_path_for, temp_tile_paths, wal_path_for,
};
use super::{BUILD_TIMEOUT, FocusContourSpec, GeoBounds, TEMP_DIR_NAME, TileKey};
use crate::settings_store;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

pub fn find_prebuilt_vrt(srtm_root: &Path) -> Option<PathBuf> {
    let parent = srtm_root.parent()?;
    // Try the canonical name first, then any .vrt in the parent directory.
    let canonical = parent.join(format!("{}.vrt", srtm_root.file_name()?.to_string_lossy()));
    if canonical.exists() {
        return Some(canonical);
    }
    std::fs::read_dir(parent).ok()?.find_map(|e| {
        let p = e.ok()?.path();
        (p.extension()?.to_str() == Some("vrt")).then_some(p)
    })
}

pub fn find_gebco_topography_tiles(data_root: &Path) -> Vec<PathBuf> {
    let root = data_root
        .join("GEBCO")
        .join("gebco_2025_sub_ice_topo_geotiff");
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut tiles: Vec<_> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.extension().and_then(|e| e.to_str()) == Some("tif")
                && p.file_name()
                    .and_then(|s| s.to_str())
                    .is_some_and(|s| s.starts_with("gebco_2025_sub_ice_"))
        })
        .collect();
    tiles.sort();
    tiles
}

pub fn find_all_srtm_tiles(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.extension().and_then(|e| e.to_str()) == Some("tif")
                && p.file_stem()
                    .and_then(|s| s.to_str())
                    .map(|s| {
                        // N/S + 2 digits + E/W + 3 digits, e.g. N35E034
                        s.len() == 7
                            && matches!(s.as_bytes()[0], b'N' | b'S')
                            && s[1..3].bytes().all(|b| b.is_ascii_digit())
                            && matches!(s.as_bytes()[3], b'E' | b'W')
                            && s[4..7].bytes().all(|b| b.is_ascii_digit())
                    })
                    .unwrap_or(false)
        })
        .collect()
}

/// Build the global land overview from either a pre-existing VRT or a list
/// of individual SRTM tiles.  Exactly one of `prebuilt_vrt` / `tiles` must
/// be non-empty/non-None.
pub fn build_global_overview(
    prebuilt_vrt: Option<&Path>,
    tiles: &[PathBuf],
    tmp_tif: &Path,
    tmp_gpkg: &Path,
    output_path: &Path,
) -> Option<()> {
    if shutdown_requested().load(Ordering::Relaxed) {
        return None;
    }
    let tmp_dir = tmp_tif.parent()?;
    fs::create_dir_all(tmp_dir).ok()?;

    // Resolve the VRT to warp from: either the pre-built one or one we
    // construct from the tile list.
    let built_vrt: Option<PathBuf>;
    let warp_source: &Path = if let Some(vrt) = prebuilt_vrt {
        built_vrt = None;
        vrt
    } else {
        // Write tile paths to a text file to avoid ARG_MAX limits and the
        // "too many open files" error gdalwarp hits with thousands of args.
        let tile_list_path = tmp_dir.join("global_tile_list.txt");
        {
            use std::io::Write as _;
            let mut f = fs::File::create(&tile_list_path).ok()?;
            for tile in tiles {
                writeln!(f, "{}", tile.display()).ok()?;
            }
        }

        let tmp_vrt = tmp_dir.join("global_overview.tmp.vrt");
        let mut cmd = Command::new(gdal_tool_path("gdalbuildvrt"));
        cmd.args(["-q", "-input_file_list"]);
        cmd.arg(&tile_list_path);
        cmd.arg(&tmp_vrt);
        run_command_with_timeout(
            cmd,
            "gdalbuildvrt (global overview)",
            Duration::from_secs(120),
        )
        .ok()?;
        let _ = fs::remove_file(&tile_list_path);

        if shutdown_requested().load(Ordering::Relaxed) {
            let _ = fs::remove_file(&tmp_vrt);
            return None;
        }
        built_vrt = Some(tmp_vrt);
        built_vrt.as_deref().unwrap()
    };

    // Merge into a 0.2°/pixel (≈22 km) global mosaic.
    // -dstnodata -32768 keeps ocean/gap areas from producing contours.
    // Use half the available cores so the machine stays responsive.
    let half_cpus = (std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        / 2)
    .max(1)
    .to_string();
    let mut cmd = Command::new(gdal_tool_path("gdalwarp"));
    cmd.args([
        "-q",
        "-overwrite",
        "-multi",
        "-wo",
        &format!("NUM_THREADS={half_cpus}"),
        "-wm",
        "1024",
        "-r",
        "average",
        "-tr",
        "0.2",
        "0.2",
        "-te",
        "-180",
        "-60",
        "180",
        "84",
        "-dstnodata",
        "-32768",
        "-co",
        "COMPRESS=LZW",
        "-co",
        "TILED=YES",
        "-co",
        "BLOCKXSIZE=512",
        "-co",
        "BLOCKYSIZE=512",
    ]);
    cmd.arg(warp_source);
    cmd.arg(tmp_tif);
    run_command_with_timeout(cmd, "gdalwarp (global overview)", Duration::from_secs(600)).ok()?;
    if let Some(ref vrt) = built_vrt {
        let _ = fs::remove_file(vrt);
    }

    if shutdown_requested().load(Ordering::Relaxed) {
        let _ = fs::remove_file(tmp_tif);
        return None;
    }

    // Contour at 500 m interval; -snodata skips the nodata cells.
    let mut cmd = Command::new(gdal_tool_path("gdal_contour"));
    cmd.args([
        "-q",
        "-f",
        "GPKG",
        "-a",
        "elevation_m",
        "-i",
        "500",
        "-snodata",
        "-32768",
        "-nln",
        "contour",
    ]);
    cmd.arg(tmp_tif);
    cmd.arg(tmp_gpkg);
    run_command_with_timeout(
        cmd,
        "gdal_contour (global overview)",
        Duration::from_secs(300),
    )
    .ok()?;

    // fs::rename fails across filesystems; fall back to copy+delete.
    if fs::rename(tmp_gpkg, output_path).is_err() {
        fs::copy(tmp_gpkg, output_path).ok()?;
        let _ = fs::remove_file(tmp_gpkg);
    }
    let _ = fs::remove_file(tmp_tif);
    Some(())
}

pub fn build_global_coastline(
    tiles: &[PathBuf],
    tmp_vrt: &Path,
    tmp_gpkg: &Path,
    output_path: &Path,
) -> Option<()> {
    if shutdown_requested().load(Ordering::Relaxed) {
        return None;
    }

    let tmp_dir = tmp_vrt.parent()?;
    fs::create_dir_all(tmp_dir).ok()?;
    let _ = fs::remove_file(tmp_vrt);
    let _ = fs::remove_file(tmp_gpkg);
    let _ = fs::remove_file(output_path);

    let mut buildvrt = Command::new(gdal_tool_path("gdalbuildvrt"));
    buildvrt.arg("-q");
    buildvrt.arg(tmp_vrt);
    for tile in tiles {
        buildvrt.arg(tile);
    }
    run_command_with_timeout(
        buildvrt,
        "gdalbuildvrt (global coastline)",
        Duration::from_secs(180),
    )
    .ok()?;

    if shutdown_requested().load(Ordering::Relaxed) {
        let _ = fs::remove_file(tmp_vrt);
        return None;
    }

    let mut contour = Command::new(gdal_tool_path("gdal_contour"));
    contour.args([
        "-q",
        "-f",
        "GPKG",
        "-a",
        "elevation_m",
        "-fl",
        "0",
        "-nln",
        "contour",
    ]);
    contour.arg(tmp_vrt);
    contour.arg(tmp_gpkg);
    run_command_with_timeout(
        contour,
        "gdal_contour (global coastline)",
        Duration::from_secs(180),
    )
    .ok()?;

    if shutdown_requested().load(Ordering::Relaxed) {
        let _ = fs::remove_file(tmp_vrt);
        let _ = fs::remove_file(tmp_gpkg);
        return None;
    }

    fs::rename(tmp_gpkg, output_path).ok()?;
    let _ = fs::remove_file(tmp_vrt);
    let _ = fs::remove_file(journal_path_for(output_path));
    let _ = fs::remove_file(wal_path_for(output_path));
    let _ = fs::remove_file(shm_path_for(output_path));
    Some(())
}

pub fn global_overview_building() -> &'static AtomicBool {
    static BUILDING: OnceLock<AtomicBool> = OnceLock::new();
    BUILDING.get_or_init(|| AtomicBool::new(false))
}

pub fn global_coastline_building() -> &'static AtomicBool {
    static BUILDING: OnceLock<AtomicBool> = OnceLock::new();
    BUILDING.get_or_init(|| AtomicBool::new(false))
}

pub fn shutdown_requested() -> &'static AtomicBool {
    static SHUTDOWN: OnceLock<AtomicBool> = OnceLock::new();
    SHUTDOWN.get_or_init(|| AtomicBool::new(false))
}

pub fn active_children() -> &'static Mutex<HashSet<u32>> {
    static ACTIVE: OnceLock<Mutex<HashSet<u32>>> = OnceLock::new();
    ACTIVE.get_or_init(|| Mutex::new(HashSet::new()))
}

pub fn gdal_tool_path(tool: &str) -> PathBuf {
    settings_store::resolve_gdal_tool(tool)
}

pub fn run_command(command: Command, label: &str) -> std::io::Result<()> {
    run_command_with_timeout(command, label, BUILD_TIMEOUT)
}

pub fn run_command_with_timeout(
    mut command: Command,
    label: &str,
    timeout: Duration,
) -> std::io::Result<()> {
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

        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            if let Ok(mut guard) = active_children().lock() {
                guard.remove(&pid);
            }
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("{label} timed out after {:?}", timeout),
            ));
        }

        std::thread::sleep(Duration::from_millis(150));
    }
}

pub fn build_focus_contours(
    srtm_root: &Path,
    cache_root: &Path,
    cache_db_path: &Path,
    tile: TileKey,
    bounds: GeoBounds,
    spec: FocusContourSpec,
) -> Option<()> {
    if shutdown_requested().load(Ordering::Relaxed) {
        return None;
    }

    let (tmp_tif_path, tmp_gpkg_path) = temp_tile_paths(cache_root, tile);
    cleanup_temp_tile_artifacts(&tmp_tif_path, &tmp_gpkg_path);
    let tiles = tile_paths_for_bounds(srtm_root, bounds);
    if tiles.is_empty() {
        return None;
    }

    if let Some(parent) = tmp_tif_path.parent() {
        fs::create_dir_all(parent).ok()?;
    }
    run_gdalwarp(&tiles, &tmp_tif_path, bounds, spec).ok()?;

    if shutdown_requested().load(Ordering::Relaxed) {
        cleanup_temp_tile_artifacts(&tmp_tif_path, &tmp_gpkg_path);
        return None;
    }

    run_gdal_contour(&tmp_tif_path, &tmp_gpkg_path, spec.interval_m).ok()?;
    import_tile_into_cache(cache_db_path, tile, &tmp_gpkg_path).ok()?;

    // Piggyback: extract 0m coastline from the same warped TIF while we have it.
    let tmp_coast_gpkg_path = cache_root.join(TEMP_DIR_NAME).join(format!(
        "z{}_lat{}_lon{}.coast.tmp.gpkg",
        tile.zoom_bucket, tile.lat_bucket, tile.lon_bucket
    ));
    if run_gdal_coastline_0m(&tmp_tif_path, &tmp_coast_gpkg_path).is_ok() {
        let _ = import_coastline_into_cache(cache_db_path, tile, &tmp_coast_gpkg_path);
    }
    let _ = fs::remove_file(&tmp_coast_gpkg_path);

    cleanup_temp_tile_artifacts(&tmp_tif_path, &tmp_gpkg_path);
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

fn run_gdal_coastline_0m(input_path: &Path, output_path: &Path) -> std::io::Result<()> {
    let mut command = Command::new(gdal_tool_path("gdal_contour"));
    command.args([
        "-q",
        "-f",
        "GPKG",
        "-a",
        "elevation_m",
        "-fl",
        "0",
        "-snodata",
        "-32768",
        "-nln",
        "contour",
    ]);
    command.arg(input_path);
    command.arg(output_path);
    run_command(command, "gdal_contour (srtm coastline 0m)")
}
