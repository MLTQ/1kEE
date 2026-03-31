/// Offline pre-builder for SLDEM2015 lunar contour tiles.
///
/// The desktop app builds these on-demand from the single 22 GB SLDEM2015 JP2
/// file, which is very slow (minutes per tile).  This module pre-builds the
/// entire bbox into `lunar_focus_cache.sqlite` so the desktop finds them
/// instantly and skips its own build.
///
/// Pipeline per tile:
///   gdal_translate -projwin … -scale -18000 22000 -9000 11000 → Float32 GeoTIFF
///   gdal_contour -a elevation_m -i {interval_m}               → GPKG
///   import_tile()                                              → lunar_focus_cache.sqlite
use crate::contours::{
    ContourBuildProgress, GeoBounds, TileKey, bucket_range, import_tile, open_cache_db,
    resolve_gdal_tool, tile_exists,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

// ── Lunar zoom specs (matches desktop zoom::lunar_spec_for_zoom exactly) ─────

#[derive(Clone, Copy)]
pub struct LunarSpec {
    pub half_extent_deg: f32,
    pub raster_size: u32,
    pub interval_m: i32,
    pub zoom_bucket: i32,
}

pub fn all_lunar_specs() -> [LunarSpec; 5] {
    [
        LunarSpec {
            half_extent_deg: 3.6,
            raster_size: 384,
            interval_m: 1000,
            zoom_bucket: 0,
        },
        LunarSpec {
            half_extent_deg: 2.2,
            raster_size: 512,
            interval_m: 500,
            zoom_bucket: 1,
        },
        LunarSpec {
            half_extent_deg: 1.4,
            raster_size: 576,
            interval_m: 200,
            zoom_bucket: 2,
        },
        LunarSpec {
            half_extent_deg: 0.9,
            raster_size: 640,
            interval_m: 100,
            zoom_bucket: 3,
        },
        LunarSpec {
            half_extent_deg: 0.55,
            raster_size: 704,
            interval_m: 50,
            zoom_bucket: 4,
        },
    ]
}

const SOURCE_CHUNK_CENTER_STEP_DEG: f32 = 4.0;
const SOURCE_CHUNK_HALF_EXTENT_DEG: f32 = 6.0;
const SOURCE_CHUNK_DIR_NAME: &str = "lunar_source_chunks";

// ── Command ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct LunarBuildCommand {
    pub jp2_path: PathBuf,
    pub cache_db_path: PathBuf, // path to lunar_focus_cache.sqlite
    pub tmp_dir: Option<PathBuf>,
    pub min_lat: f32,
    pub max_lat: f32,
    pub min_lon: f32,
    pub max_lon: f32,
    pub zoom_buckets: Vec<i32>, // subset of 0..=4
    pub gdal_bin_dir: PathBuf,  // "" = use Homebrew / $PATH
}

// ── GDAL helpers ──────────────────────────────────────────────────────────────

fn run_gdal_with_timeout(mut cmd: Command, label: &str, timeout: Duration) -> std::io::Result<()> {
    let start = Instant::now();
    let mut child = cmd.spawn()?;
    loop {
        if let Some(status) = child.try_wait()? {
            return if status.success() {
                Ok(())
            } else {
                Err(std::io::Error::other(format!(
                    "{label} failed with status {status}"
                )))
            };
        }
        if start.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("{label} timed out after {:?}", timeout),
            ));
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// Like `run_gdal` in contours.rs but with a 10-minute timeout — reading a
/// geographic subregion from the 22 GB SLDEM JP2 can take several minutes.
fn run_gdal_jp2(cmd: Command, label: &str) -> std::io::Result<()> {
    run_gdal_with_timeout(cmd, label, Duration::from_secs(600))
}

fn run_gdal_contour_lunar(
    gdal_contour: &Path,
    in_tif: &Path,
    out_gpkg: &Path,
    interval_m: i32,
) -> std::io::Result<()> {
    let mut cmd = Command::new(gdal_contour);
    cmd.args([
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
    cmd.arg(in_tif).arg(out_gpkg);
    // Contour generation at fine intervals (50 m) on lunar terrain can take ~60 s
    let timeout = Duration::from_secs(300);
    let start = Instant::now();
    let mut child = cmd.spawn()?;
    loop {
        if let Some(status) = child.try_wait()? {
            return if status.success() {
                Ok(())
            } else {
                Err(std::io::Error::other(format!(
                    "gdal_contour failed with {status}"
                )))
            };
        }
        if start.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "gdal_contour timed out",
            ));
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}

fn cleanup(paths: &[&Path]) {
    for p in paths {
        let _ = fs::remove_file(p);
    }
}

#[derive(Clone)]
struct SourceChunk {
    path: PathBuf,
    bounds: GeoBounds,
    raster_size: u32,
}

fn source_chunk_root(cache_db_path: &Path) -> PathBuf {
    cache_db_path
        .parent()
        .unwrap_or(Path::new("."))
        .join(SOURCE_CHUNK_DIR_NAME)
}

fn source_chunk_for_bounds(
    cache_db_path: &Path,
    bounds: GeoBounds,
    spec: LunarSpec,
) -> SourceChunk {
    let center_lat = (bounds.min_lat + bounds.max_lat) * 0.5;
    let center_lon = (bounds.min_lon + bounds.max_lon) * 0.5;
    let lat_bucket = (center_lat / SOURCE_CHUNK_CENTER_STEP_DEG).round() as i32;
    let lon_bucket = (center_lon / SOURCE_CHUNK_CENTER_STEP_DEG).round() as i32;
    let chunk_center_lat = lat_bucket as f32 * SOURCE_CHUNK_CENTER_STEP_DEG;
    let chunk_center_lon = lon_bucket as f32 * SOURCE_CHUNK_CENTER_STEP_DEG;
    let pixels_per_degree = spec.raster_size as f32 / (spec.half_extent_deg * 2.0);
    let chunk_span = SOURCE_CHUNK_HALF_EXTENT_DEG * 2.0;
    let raster_size = (chunk_span * pixels_per_degree).ceil() as u32;
    let dir = source_chunk_root(cache_db_path).join(format!("z{}", spec.zoom_bucket));
    let file_name = format!("lat{lat_bucket:+04}_lon{lon_bucket:+04}.tif");
    SourceChunk {
        path: dir.join(file_name),
        bounds: GeoBounds {
            min_lat: (chunk_center_lat - SOURCE_CHUNK_HALF_EXTENT_DEG).clamp(-89.999, 89.999),
            max_lat: (chunk_center_lat + SOURCE_CHUNK_HALF_EXTENT_DEG).clamp(-89.999, 89.999),
            min_lon: chunk_center_lon - SOURCE_CHUNK_HALF_EXTENT_DEG,
            max_lon: chunk_center_lon + SOURCE_CHUNK_HALF_EXTENT_DEG,
        },
        raster_size: raster_size.max(spec.raster_size),
    }
}

fn temp_sibling(path: &Path, suffix: &str) -> PathBuf {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("chunk");
    path.with_file_name(format!("{stem}.{suffix}"))
}

fn persist_temp_file(tmp_path: &Path, final_path: &Path) -> std::io::Result<()> {
    if fs::rename(tmp_path, final_path).is_ok() {
        return Ok(());
    }
    fs::copy(tmp_path, final_path)?;
    fs::remove_file(tmp_path)?;
    Ok(())
}

fn ensure_source_chunk(
    jp2_path: &Path,
    cache_db_path: &Path,
    gdal_translate: &Path,
    spec: LunarSpec,
    bounds: GeoBounds,
) -> Result<SourceChunk, String> {
    let chunk = source_chunk_for_bounds(cache_db_path, bounds, spec);
    if chunk.path.exists() {
        return Ok(chunk);
    }

    let Some(parent) = chunk.path.parent() else {
        return Err(format!(
            "Invalid lunar source chunk path: {}",
            chunk.path.display()
        ));
    };
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;

    let tmp_chunk = temp_sibling(&chunk.path, &format!("{}.tmp.tif", std::process::id()));
    let _ = fs::remove_file(&tmp_chunk);

    let mut translate = Command::new(gdal_translate);
    translate.args([
        "-q",
        "-projwin",
        &chunk.bounds.min_lon.to_string(),
        &chunk.bounds.max_lat.to_string(),
        &chunk.bounds.max_lon.to_string(),
        &chunk.bounds.min_lat.to_string(),
        "-outsize",
        &chunk.raster_size.to_string(),
        &chunk.raster_size.to_string(),
        "-scale",
        "-18000",
        "22000",
        "-9000",
        "11000",
        "-ot",
        "Int16",
        "-of",
        "GTiff",
        "-co",
        "TILED=YES",
        "-co",
        "COMPRESS=LZW",
        "-co",
        "PREDICTOR=2",
        "-co",
        "BLOCKXSIZE=512",
        "-co",
        "BLOCKYSIZE=512",
    ]);
    translate.arg(jp2_path).arg(&tmp_chunk);
    run_gdal_jp2(translate, "gdal_translate (lunar source chunk)").map_err(|e| e.to_string())?;
    persist_temp_file(&tmp_chunk, &chunk.path).map_err(|e| e.to_string())?;
    Ok(chunk)
}

// ── Main entry point ─────────────────────────────────────────────────────────

/// Pre-build lunar contour tiles for the given bounding box and zoom buckets.
///
/// Writes into `command.cache_db_path` (the desktop reads this as
/// `Derived/terrain/lunar_focus_cache.sqlite`).
///
/// Tiles already present in the DB are skipped.  Coverage is clipped to ±60°
/// latitude (the extent of SLDEM2015).
pub fn build_lunar_contour_tiles(
    command: LunarBuildCommand,
    progress: &mut dyn FnMut(ContourBuildProgress),
) -> Result<String, String> {
    // ── Validate inputs ───────────────────────────────────────────────────────
    if !command.jp2_path.exists() {
        return Err(format!(
            "SLDEM JP2 not found: {}",
            command.jp2_path.display()
        ));
    }
    let gdal_translate = resolve_gdal_tool(&command.gdal_bin_dir, "gdal_translate");
    let gdal_contour = resolve_gdal_tool(&command.gdal_bin_dir, "gdal_contour");
    for (tool, name) in [
        (&gdal_translate, "gdal_translate"),
        (&gdal_contour, "gdal_contour"),
    ] {
        match Command::new(tool).arg("--version").output() {
            Ok(out) if out.status.success() => {
                let ver = String::from_utf8_lossy(&out.stdout);
                progress(ContourBuildProgress::info(
                    "Startup",
                    0.0,
                    format!("{name}: {}", ver.trim()),
                ));
            }
            Ok(_) => return Err(format!("{name} at '{}' returned an error", tool.display())),
            Err(e) => {
                return Err(format!(
                    "Could not launch {name} at '{}': {e}. Set GDAL bin dir.",
                    tool.display()
                ));
            }
        }
    }

    let tmp_dir = command.tmp_dir.clone().unwrap_or_else(|| {
        command
            .cache_db_path
            .parent()
            .unwrap_or(Path::new("."))
            .join("lunar_focus_tmp")
    });
    fs::create_dir_all(&tmp_dir).map_err(|e| e.to_string())?;
    open_cache_db(&command.cache_db_path).map_err(|e| e.to_string())?;

    let specs = all_lunar_specs();
    let selected: Vec<LunarSpec> = specs
        .iter()
        .filter(|s| command.zoom_buckets.contains(&s.zoom_bucket))
        .copied()
        .collect();

    if selected.is_empty() {
        return Err("No zoom buckets selected.".to_owned());
    }

    // ── Collect work ──────────────────────────────────────────────────────────
    progress(ContourBuildProgress::info(
        "Planning",
        0.0,
        "Scanning tiles…",
    ));

    struct TileWork {
        tile: TileKey,
        bounds: GeoBounds,
        spec: LunarSpec,
    }

    // SLDEM2015 covers ±60° latitude only
    const SLDEM_LAT_LIMIT: f32 = 60.0;
    let req_min_lat = command.min_lat.max(-SLDEM_LAT_LIMIT);
    let req_max_lat = command.max_lat.min(SLDEM_LAT_LIMIT);

    if req_min_lat >= req_max_lat {
        return Err(format!(
            "Requested bbox is entirely outside SLDEM coverage (±{SLDEM_LAT_LIMIT}°)."
        ));
    }

    let conn = open_cache_db(&command.cache_db_path).map_err(|e| e.to_string())?;
    let mut work: Vec<TileWork> = Vec::new();
    let mut skipped = 0usize;

    for spec in &selected {
        let step = spec.half_extent_deg * 0.45;
        for lat_bucket in bucket_range(req_min_lat, req_max_lat, step) {
            let center_lat = (lat_bucket as f32 * step).clamp(-89.999, 89.999);
            // Skip tiles whose centre is outside SLDEM coverage
            if center_lat.abs() > SLDEM_LAT_LIMIT + spec.half_extent_deg {
                continue;
            }
            for lon_bucket in bucket_range(command.min_lon, command.max_lon, step) {
                let tile = TileKey {
                    zoom_bucket: spec.zoom_bucket,
                    lat_bucket,
                    lon_bucket,
                };
                if tile_exists(&conn, tile) {
                    skipped += 1;
                    continue;
                }
                let center_lon = lon_bucket as f32 * step;
                let bounds = GeoBounds {
                    min_lat: (center_lat - spec.half_extent_deg).clamp(-89.999, 89.999),
                    max_lat: (center_lat + spec.half_extent_deg).clamp(-89.999, 89.999),
                    min_lon: center_lon - spec.half_extent_deg,
                    max_lon: center_lon + spec.half_extent_deg,
                };
                work.push(TileWork {
                    tile,
                    bounds,
                    spec: *spec,
                });
            }
        }
    }
    drop(conn);

    let total = work.len() + skipped;
    let to_build = work.len();
    progress(ContourBuildProgress::info(
        "Planning",
        0.0,
        format!("{to_build} tiles to build, {skipped} already cached, {total} total"),
    ));
    progress(ContourBuildProgress::info(
        "Planning",
        0.0,
        format!(
            "Reusing persistent lunar source chunks in {}",
            source_chunk_root(&command.cache_db_path).display()
        ),
    ));

    if work.is_empty() {
        return Ok(format!(
            "Lunar contours complete: 0 built, {skipped} already cached."
        ));
    }

    // ── Sequential build (all tiles read from the same JP2 file) ─────────────
    // Parallel reads from the same large JP2 cause heavy I/O contention and
    // make each individual read slower.  Sequential is simpler and more reliable.
    let mut built = 0usize;
    let mut errors = 0usize;

    for (idx, item) in work.iter().enumerate() {
        let fraction = idx as f32 / to_build as f32;
        let center_lat =
            (item.tile.lat_bucket as f32 * item.spec.half_extent_deg * 0.45).clamp(-89.999, 89.999);
        let center_lon = item.tile.lon_bucket as f32 * item.spec.half_extent_deg * 0.45;
        progress(ContourBuildProgress::info(
            "Building",
            fraction,
            format!(
                "[{}/{}] z{} lat{:.2} lon{:.2} ({} m interval)",
                idx + 1,
                to_build,
                item.tile.zoom_bucket,
                center_lat,
                center_lon,
                item.spec.interval_m,
            ),
        ));

        let stem = format!(
            "z{}_lat{}_lon{}",
            item.tile.zoom_bucket, item.tile.lat_bucket, item.tile.lon_bucket
        );
        let tmp_tif = tmp_dir.join(format!("{stem}.tmp.tif"));
        let tmp_gpkg = tmp_dir.join(format!("{stem}.tmp.gpkg"));
        cleanup(&[&tmp_tif, &tmp_gpkg]);

        let chunk = match ensure_source_chunk(
            &command.jp2_path,
            &command.cache_db_path,
            &gdal_translate,
            item.spec,
            item.bounds,
        ) {
            Ok(chunk) => chunk,
            Err(e) => {
                progress(ContourBuildProgress::error(
                    "Building",
                    fraction,
                    format!(
                        "source chunk failed for z{} ({:.2},{:.2}): {e}",
                        item.tile.zoom_bucket, center_lat, center_lon
                    ),
                ));
                cleanup(&[&tmp_tif, &tmp_gpkg]);
                errors += 1;
                continue;
            }
        };

        // Step 1: gdal_translate — crop the cached source chunk to the exact tile
        let mut translate = Command::new(&gdal_translate);
        translate.args([
            "-q",
            "-projwin",
            &item.bounds.min_lon.to_string(),
            &item.bounds.max_lat.to_string(),
            &item.bounds.max_lon.to_string(),
            &item.bounds.min_lat.to_string(),
            "-outsize",
            &item.spec.raster_size.to_string(),
            &item.spec.raster_size.to_string(),
            "-ot",
            "Int16",
            "-of",
            "GTiff",
        ]);
        translate.arg(&chunk.path).arg(&tmp_tif);

        if let Err(e) = run_gdal_with_timeout(
            translate,
            "gdal_translate (lunar cached tile)",
            Duration::from_secs(120),
        ) {
            progress(ContourBuildProgress::error(
                "Building",
                fraction,
                format!(
                    "gdal_translate failed for z{} ({:.2},{:.2}): {e}",
                    item.tile.zoom_bucket, center_lat, center_lon
                ),
            ));
            cleanup(&[&tmp_tif, &tmp_gpkg]);
            errors += 1;
            continue;
        }

        // Step 2: gdal_contour
        if let Err(e) =
            run_gdal_contour_lunar(&gdal_contour, &tmp_tif, &tmp_gpkg, item.spec.interval_m)
        {
            progress(ContourBuildProgress::error(
                "Building",
                fraction,
                format!("gdal_contour failed: {e}"),
            ));
            cleanup(&[&tmp_tif, &tmp_gpkg]);
            errors += 1;
            continue;
        }

        // Step 3: import into DB
        if let Err(e) = import_tile(&command.cache_db_path, item.tile, &tmp_gpkg) {
            progress(ContourBuildProgress::error(
                "Building",
                fraction,
                format!("DB import failed: {e}"),
            ));
            cleanup(&[&tmp_tif, &tmp_gpkg]);
            errors += 1;
            continue;
        }

        cleanup(&[&tmp_tif, &tmp_gpkg]);
        built += 1;
        progress(ContourBuildProgress::built(
            "Building",
            (idx + 1) as f32 / to_build as f32,
            format!(
                "Built tile z{} ({:.2},{:.2})",
                item.tile.zoom_bucket, center_lat, center_lon
            ),
            (
                item.bounds.min_lat,
                item.bounds.max_lat,
                item.bounds.min_lon,
                item.bounds.max_lon,
            ),
        ));
    }

    let summary = format!(
        "Lunar contours complete: {built} built, {skipped} already cached, {errors} errors, {total} total tiles"
    );
    Ok(summary)
}
