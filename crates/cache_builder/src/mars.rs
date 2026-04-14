/// Offline pre-builder for Mars terrain contour tiles.
///
/// Sources:
///   CTX DTM tiles  (`<data_root>/mars_data/<pair>/*-DEM-geoid-adj.tif`) — ~20 m/px
///   MOLA MEGDR     (`<data_root>/MOLA/megt*.img`) — global ~463 m/px fallback
///
/// Build rules per tile:
///   contour_count > 0          → skip (tile already has real data)
///   contour_count = 0, no MOLA → skip (previously empty, nothing to upgrade)
///   contour_count = 0, MOLA OK → rebuild (upgrade from MOLA)
///   not in manifest            → build (CTX if available, MOLA otherwise)
///
/// Mars uses the same five zoom specs as Lunar.
use crate::contours::{
    ContourBuildProgress, FocusContourSpec, GeoBounds, TileKey, bucket_range, import_tile,
    open_cache_db, resolve_gdal_tool, tile_contour_count,
};
use crate::lunar::all_lunar_specs;
use rayon::prelude::*;
use rusqlite::params;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const MARS_SRS: &str = "+proj=longlat +R=3396190 +no_defs";
const MARS_NODATA: &str = "-32767";
/// Spatial buffer added when querying the CTX index — CTX pairs can span
/// ~100–300 km (~1–3°) from their nominal centre coordinate.
const SPATIAL_BUFFER_DEG: f32 = 3.0;

// ── Command ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct MarsBuildCommand {
    /// Mars data root; must contain `mars_data/` (CTX DEMs) and/or `MOLA/` (fallback tiles).
    pub data_root: PathBuf,
    pub cache_db_path: PathBuf,
    pub tmp_dir: Option<PathBuf>,
    pub min_lat: f32,
    pub max_lat: f32,
    pub min_lon: f32,
    pub max_lon: f32,
    /// Which zoom buckets (0–4) to build.
    pub zoom_buckets: Vec<i32>,
    pub gdal_bin_dir: PathBuf,
}

// ── CTX spatial index ─────────────────────────────────────────────────────────

struct MarsIndexEntry {
    lat: f32,
    lon: f32,
    dem_path: PathBuf,
}

/// Parse the approximate lat/lon centre from a CTX DTM directory name.
///
/// Names follow `<img1>__<img2>` where each image name ends with a
/// lat/lon suffix like `04S063W` (2-digit lat + hemisphere + 3-digit west-lon + hemisphere).
fn parse_ctx_center(dir_name: &str) -> Option<(f32, f32)> {
    let first = dir_name.split("__").next()?;
    let suffix = first.rsplit('_').next()?;
    if suffix.len() != 7 {
        return None;
    }
    let lat_deg: f32 = suffix[0..2].parse().ok()?;
    let lat = match suffix.as_bytes()[2] {
        b'S' => -lat_deg,
        b'N' => lat_deg,
        _ => return None,
    };
    let lon_deg: f32 = suffix[3..6].parse().ok()?;
    let lon = match suffix.as_bytes()[6] {
        b'W' => {
            let l = -lon_deg;
            if l < -180.0 { l + 360.0 } else { l }
        }
        b'E' => lon_deg,
        _ => return None,
    };
    Some((lat, lon))
}

fn build_ctx_index(data_root: &Path) -> Vec<MarsIndexEntry> {
    let mars_data = data_root.join("mars_data");
    let Ok(entries) = fs::read_dir(&mars_data) else {
        return Vec::new();
    };
    let mut index = Vec::new();
    for entry in entries.flatten() {
        let dir_path = entry.path();
        if !dir_path.is_dir() {
            continue;
        }
        let Some(dir_name) = dir_path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some((lat, lon)) = parse_ctx_center(dir_name) else {
            continue;
        };
        let dem_path = dir_path.join(format!("{dir_name}-DEM-geoid-adj.tif"));
        if dem_path.exists() {
            index.push(MarsIndexEntry { lat, lon, dem_path });
        }
    }
    index
}

fn find_ctx_tiles_for_bounds(index: &[MarsIndexEntry], bounds: GeoBounds) -> Vec<PathBuf> {
    let min_lat = bounds.min_lat - SPATIAL_BUFFER_DEG;
    let max_lat = bounds.max_lat + SPATIAL_BUFFER_DEG;
    let min_lon = bounds.min_lon - SPATIAL_BUFFER_DEG;
    let max_lon = bounds.max_lon + SPATIAL_BUFFER_DEG;
    index
        .iter()
        .filter(|e| {
            e.lat >= min_lat && e.lat <= max_lat && e.lon >= min_lon && e.lon <= max_lon
        })
        .map(|e| e.dem_path.clone())
        .collect()
}

// ── MOLA VRT ─────────────────────────────────────────────────────────────────

fn find_mola_tiles(data_root: &Path) -> Vec<PathBuf> {
    let mola_dir = data_root.join("MOLA");
    let Ok(entries) = fs::read_dir(&mola_dir) else {
        return Vec::new();
    };
    let mut tiles: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            // Use .lbl / .LBL (PDS3 label) — the correct GDAL entry point.
            // The PDS3 driver reads the label first, then locates the .img data.
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("lbl"))
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.to_ascii_lowercase().starts_with("megt"))
        })
        .collect();
    tiles.sort();
    tiles
}

fn ensure_mola_vrt(gdalbuildvrt: &Path, mola_tiles: &[PathBuf]) -> Option<PathBuf> {
    if mola_tiles.is_empty() {
        return None;
    }
    let vrt_path = mola_tiles.first()?.parent()?.join("mola_megdr.vrt");
    if vrt_path.exists() {
        return Some(vrt_path);
    }
    let mut cmd = Command::new(gdalbuildvrt);
    cmd.arg("-q").arg(&vrt_path);
    for tile in mola_tiles {
        cmd.arg(tile);
    }
    run_gdal_timed(cmd, "gdalbuildvrt (MOLA mosaic)", Duration::from_secs(30)).ok()?;
    vrt_path.exists().then_some(vrt_path)
}

// ── GDAL helpers ──────────────────────────────────────────────────────────────

fn run_gdal_timed(mut cmd: Command, label: &str, timeout: Duration) -> std::io::Result<()> {
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
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn run_gdalwarp_mars(
    gdalwarp: &Path,
    sources: &[PathBuf],
    out_tif: &Path,
    bounds: GeoBounds,
    spec: &FocusContourSpec,
) -> std::io::Result<()> {
    let mut cmd = Command::new(gdalwarp);
    cmd.args([
        "-q",
        "-overwrite",
        "-t_srs",
        MARS_SRS,
        "-r",
        "bilinear",
        "-dstnodata",
        MARS_NODATA,
        "-te",
        &format!("{:.6}", bounds.min_lon),
        &format!("{:.6}", bounds.min_lat),
        &format!("{:.6}", bounds.max_lon),
        &format!("{:.6}", bounds.max_lat),
        "-ts",
        &spec.raster_size.to_string(),
        &spec.raster_size.to_string(),
    ]);
    for s in sources {
        cmd.arg(s);
    }
    cmd.arg(out_tif);
    // CTX tiles with multiple sources can take longer; MOLA is fast.
    run_gdal_timed(cmd, "gdalwarp (mars)", Duration::from_secs(300))
}

fn run_gdal_contour_mars(
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
        "-snodata",
        MARS_NODATA,
        "-nln",
        "contour",
    ]);
    cmd.arg(in_tif).arg(out_gpkg);
    run_gdal_timed(cmd, "gdal_contour (mars)", Duration::from_secs(120))
}

fn cleanup(paths: &[&Path]) {
    for p in paths {
        let _ = fs::remove_file(p);
    }
}

fn temp_tile_path(tmp_dir: &Path, tile: TileKey, suffix: &str) -> PathBuf {
    tmp_dir.join(format!(
        "mars_z{}_lat{}_lon{}.{}",
        tile.zoom_bucket, tile.lat_bucket, tile.lon_bucket, suffix
    ))
}

fn mark_tile_empty(cache_db_path: &Path, tile: TileKey) {
    let Ok(conn) = rusqlite::Connection::open(cache_db_path) else {
        return;
    };
    let _ = conn.execute(
        "INSERT OR REPLACE INTO contour_tile_manifest
             (zoom_bucket, lat_bucket, lon_bucket, contour_count, built_at)
         VALUES (?1, ?2, ?3, 0, unixepoch())",
        params![tile.zoom_bucket, tile.lat_bucket, tile.lon_bucket],
    );
}

// ── Main entry point ─────────────────────────────────────────────────────────

/// Pre-build Mars contour tiles for the given bounding box and zoom buckets.
///
/// Writes into `command.cache_db_path` (the desktop reads this as
/// `Derived/terrain/mars_focus_cache.sqlite`).
pub fn build_mars_contour_tiles(
    command: MarsBuildCommand,
    progress: &mut dyn FnMut(ContourBuildProgress),
) -> Result<String, String> {
    if !command.data_root.exists() {
        return Err(format!(
            "Mars data root not found: {}",
            command.data_root.display()
        ));
    }

    // ── Validate GDAL tools ───────────────────────────────────────────────────
    let gdalwarp = resolve_gdal_tool(&command.gdal_bin_dir, "gdalwarp");
    let gdal_contour = resolve_gdal_tool(&command.gdal_bin_dir, "gdal_contour");
    let gdalbuildvrt = resolve_gdal_tool(&command.gdal_bin_dir, "gdalbuildvrt");

    for (tool, name) in [
        (&gdalwarp, "gdalwarp"),
        (&gdal_contour, "gdal_contour"),
        (&gdalbuildvrt, "gdalbuildvrt"),
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
            Ok(_) => {
                return Err(format!(
                    "{name} at '{}' returned an error on --version",
                    tool.display()
                ));
            }
            Err(e) => {
                return Err(format!(
                    "Could not launch {name} at '{}': {e}. Set GDAL bin dir.",
                    tool.display()
                ));
            }
        }
    }

    // ── CTX spatial index ─────────────────────────────────────────────────────
    let ctx_index = build_ctx_index(&command.data_root);
    progress(ContourBuildProgress::info(
        "Startup",
        0.0,
        format!("CTX index: {} DEMs found", ctx_index.len()),
    ));

    // ── MOLA VRT ──────────────────────────────────────────────────────────────
    let mola_tiles = find_mola_tiles(&command.data_root);
    let mola_vrt: Option<PathBuf> = if !mola_tiles.is_empty() {
        match ensure_mola_vrt(&gdalbuildvrt, &mola_tiles) {
            Some(vrt) => {
                progress(ContourBuildProgress::info(
                    "Startup",
                    0.0,
                    format!(
                        "MOLA VRT ready ({} tiles): {}",
                        mola_tiles.len(),
                        vrt.display()
                    ),
                ));
                Some(vrt)
            }
            None => {
                progress(ContourBuildProgress::error(
                    "Startup",
                    0.0,
                    "Failed to build MOLA VRT — global fallback disabled",
                ));
                None
            }
        }
    } else {
        progress(ContourBuildProgress::info(
            "Startup",
            0.0,
            "No MOLA tiles found in MOLA/ — global fallback unavailable",
        ));
        None
    };

    if ctx_index.is_empty() && mola_vrt.is_none() {
        return Err(
            "No Mars source data: no CTX DEMs in mars_data/ and no MOLA tiles in MOLA/"
                .to_owned(),
        );
    }

    // ── Temp dir & cache DB ───────────────────────────────────────────────────
    let tmp_dir = command.tmp_dir.clone().unwrap_or_else(|| {
        command
            .cache_db_path
            .parent()
            .unwrap_or(Path::new("."))
            .join("mars_focus_tmp")
    });
    fs::create_dir_all(&tmp_dir).map_err(|e| e.to_string())?;
    open_cache_db(&command.cache_db_path).map_err(|e| e.to_string())?;

    // ── Collect work ──────────────────────────────────────────────────────────
    progress(ContourBuildProgress::info("Planning", 0.0, "Scanning tiles…"));

    let specs = all_lunar_specs();
    let selected: Vec<_> = specs
        .iter()
        .filter(|s| command.zoom_buckets.contains(&s.zoom_bucket))
        .copied()
        .collect();

    if selected.is_empty() {
        return Err("No zoom buckets selected.".to_owned());
    }

    struct TileWork {
        tile: TileKey,
        bounds: GeoBounds,
        spec: crate::lunar::LunarSpec,
        ctx_sources: Vec<PathBuf>,
        mola_vrt: Option<PathBuf>,
    }

    let conn = open_cache_db(&command.cache_db_path).map_err(|e| e.to_string())?;
    let mut work: Vec<TileWork> = Vec::new();
    let mut skipped = 0usize;
    let mut marked_empty = 0usize;

    for spec in &selected {
        let step = spec.half_extent_deg * 0.45;
        for lat_bucket in bucket_range(command.min_lat, command.max_lat, step) {
            let center_lat = (lat_bucket as f32 * step).clamp(-89.999, 89.999);
            for lon_bucket in bucket_range(command.min_lon, command.max_lon, step) {
                let tile = TileKey {
                    zoom_bucket: spec.zoom_bucket,
                    lat_bucket,
                    lon_bucket,
                };

                let count = tile_contour_count(&conn, tile).unwrap_or(None);
                match count {
                    Some(c) if c > 0 => {
                        // Tile already has real contour data.
                        skipped += 1;
                        continue;
                    }
                    Some(0) if mola_vrt.is_none() => {
                        // Previously empty, no MOLA source to upgrade with.
                        skipped += 1;
                        continue;
                    }
                    _ => {}
                }

                let center_lon = lon_bucket as f32 * step;
                let bounds = GeoBounds {
                    min_lat: (center_lat - spec.half_extent_deg).clamp(-89.999, 89.999),
                    max_lat: (center_lat + spec.half_extent_deg).clamp(-89.999, 89.999),
                    min_lon: center_lon - spec.half_extent_deg,
                    max_lon: center_lon + spec.half_extent_deg,
                };

                let ctx_sources = find_ctx_tiles_for_bounds(&ctx_index, bounds);
                if ctx_sources.is_empty() && mola_vrt.is_none() {
                    // No source at all — mark empty so we don't retry every run.
                    if count.is_none() {
                        mark_tile_empty(&command.cache_db_path, tile);
                        marked_empty += 1;
                    }
                    continue;
                }

                work.push(TileWork {
                    tile,
                    bounds,
                    spec: *spec,
                    ctx_sources,
                    mola_vrt: mola_vrt.clone(),
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
        format!(
            "{to_build} tiles to build, {skipped} already cached, \
             {marked_empty} marked empty, {total} total"
        ),
    ));

    if work.is_empty() {
        return Ok(format!(
            "Mars contours complete: 0 built, {skipped} already cached."
        ));
    }

    // ── Parallel GDAL build ───────────────────────────────────────────────────
    let num_threads = (std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        / 2)
    .max(2)
    .min(8);
    progress(ContourBuildProgress::info(
        "Building",
        0.0,
        format!("Starting {num_threads} parallel GDAL workers"),
    ));

    enum Outcome {
        Built(f32, f32, f32, f32),
        Error(String),
    }

    let (tx, rx) = mpsc::channel::<Outcome>();
    let done_count = Arc::new(AtomicUsize::new(0));

    let cache_db_path = command.cache_db_path.clone();
    let tmp_dir_path = tmp_dir.clone();
    let gdalwarp_path = gdalwarp.clone();
    let gdal_contour_path = gdal_contour.clone();
    let done_arc = done_count.clone();

    std::thread::spawn(move || {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(num_threads)
            .build()
            .expect("rayon pool");

        pool.install(|| {
            work.into_par_iter()
                .map_init(
                    || (gdalwarp_path.clone(), gdal_contour_path.clone()),
                    |(gw, gc), item| {
                        let tmp_tif = temp_tile_path(&tmp_dir_path, item.tile, "tmp.tif");
                        let tmp_gpkg = temp_tile_path(&tmp_dir_path, item.tile, "tmp.gpkg");
                        cleanup(&[&tmp_tif, &tmp_gpkg]);

                        let focus_spec = FocusContourSpec {
                            half_extent_deg: item.spec.half_extent_deg,
                            raster_size: item.spec.raster_size,
                            interval_m: item.spec.interval_m,
                            zoom_bucket: item.spec.zoom_bucket,
                        };

                        // Choose source: CTX (high-res) preferred, MOLA as fallback.
                        let sources: Vec<PathBuf> = if !item.ctx_sources.is_empty() {
                            item.ctx_sources
                        } else if let Some(vrt) = item.mola_vrt {
                            vec![vrt]
                        } else {
                            return Outcome::Error(format!(
                                "z{}({},{}) no source data",
                                item.tile.zoom_bucket,
                                item.tile.lat_bucket,
                                item.tile.lon_bucket
                            ));
                        };

                        if let Err(e) =
                            run_gdalwarp_mars(gw, &sources, &tmp_tif, item.bounds, &focus_spec)
                        {
                            cleanup(&[&tmp_tif, &tmp_gpkg]);
                            return Outcome::Error(format!(
                                "z{}({},{}) warp: {e}",
                                item.tile.zoom_bucket,
                                item.tile.lat_bucket,
                                item.tile.lon_bucket
                            ));
                        }

                        if let Err(e) = run_gdal_contour_mars(
                            gc,
                            &tmp_tif,
                            &tmp_gpkg,
                            focus_spec.interval_m,
                        ) {
                            cleanup(&[&tmp_tif, &tmp_gpkg]);
                            return Outcome::Error(format!(
                                "z{}({},{}) contour: {e}",
                                item.tile.zoom_bucket,
                                item.tile.lat_bucket,
                                item.tile.lon_bucket
                            ));
                        }

                        let result = import_tile(&cache_db_path, item.tile, &tmp_gpkg);
                        cleanup(&[&tmp_tif, &tmp_gpkg]);
                        done_arc.fetch_add(1, Ordering::Relaxed);

                        match result {
                            Ok(()) => Outcome::Built(
                                item.bounds.min_lat,
                                item.bounds.max_lat,
                                item.bounds.min_lon,
                                item.bounds.max_lon,
                            ),
                            Err(e) => Outcome::Error(format!(
                                "z{}({},{}) import: {e}",
                                item.tile.zoom_bucket,
                                item.tile.lat_bucket,
                                item.tile.lon_bucket
                            )),
                        }
                    },
                )
                .for_each_with(tx, |tx, outcome| {
                    let _ = tx.send(outcome);
                });
        });
    });

    let mut built = 0usize;
    let mut errors = 0usize;
    for outcome in rx {
        let done = done_count.load(Ordering::Relaxed);
        let fraction = done as f32 / to_build.max(1) as f32;
        match outcome {
            Outcome::Built(min_lat, max_lat, min_lon, max_lon) => {
                built += 1;
                progress(ContourBuildProgress::built(
                    "Building",
                    fraction,
                    format!("{done}/{to_build} tiles built"),
                    (min_lat, max_lat, min_lon, max_lon),
                ));
            }
            Outcome::Error(msg) => {
                errors += 1;
                progress(ContourBuildProgress::error("Building", fraction, msg));
            }
        }
    }

    if let Ok(conn) = rusqlite::Connection::open(&command.cache_db_path) {
        let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)");
    }

    Ok(format!(
        "Mars contours complete: {built} built, {skipped} already cached, \
         {errors} errors, {total} total tiles"
    ))
}
