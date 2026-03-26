/// Offline contour pre-builder — Phase A (GDAL pipeline, SQLite output).
///
/// Iterates every tile in `zoom_buckets` that covers the requested bounding
/// box, skips tiles already present in `srtm_focus_cache.sqlite`, and runs
/// the same gdalwarp + gdal_contour pipeline the desktop uses on-demand.
/// The desktop will find the pre-built tiles and skip its own generation.
use rusqlite::{Connection, OptionalExtension, params};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

// ── Types (mirrors desktop srtm_focus_cache internals) ───────────────────────

#[derive(Clone, Copy)]
pub struct FocusContourSpec {
    pub half_extent_deg: f32,
    pub raster_size: u32,
    pub interval_m: i32,
    pub zoom_bucket: i32,
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct TileKey {
    pub zoom_bucket: i32,
    pub lat_bucket: i32,
    pub lon_bucket: i32,
}

#[derive(Clone, Copy)]
pub struct GeoBounds {
    pub min_lat: f32,
    pub max_lat: f32,
    pub min_lon: f32,
    pub max_lon: f32,
}

// ── Zoom spec (matches desktop zoom.rs exactly) ───────────────────────────────

pub fn all_specs() -> [FocusContourSpec; 7] {
    [
        FocusContourSpec { half_extent_deg: 3.6,  raster_size: 384, interval_m: 50, zoom_bucket: 0 },
        FocusContourSpec { half_extent_deg: 2.2,  raster_size: 512, interval_m: 25, zoom_bucket: 1 },
        FocusContourSpec { half_extent_deg: 1.4,  raster_size: 576, interval_m: 20, zoom_bucket: 2 },
        FocusContourSpec { half_extent_deg: 0.9,  raster_size: 640, interval_m: 10, zoom_bucket: 3 },
        FocusContourSpec { half_extent_deg: 0.55, raster_size: 704, interval_m: 10, zoom_bucket: 4 },
        FocusContourSpec { half_extent_deg: 0.3,  raster_size: 768, interval_m:  5, zoom_bucket: 5 },
        FocusContourSpec { half_extent_deg: 0.16, raster_size: 896, interval_m:  5, zoom_bucket: 6 },
    ]
}

// ── SQLite helpers (mirrors desktop db.rs) ────────────────────────────────────

const CACHE_DB_NAME: &str = "srtm_focus_cache.sqlite";
const TEMP_DIR_NAME: &str = "srtm_focus_tmp";

pub fn default_cache_db_path(derived_terrain_dir: &Path) -> PathBuf {
    derived_terrain_dir.join(CACHE_DB_NAME)
}

fn open_cache_db(path: &Path) -> rusqlite::Result<Connection> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).ok();
    }
    let conn = Connection::open(path)?;
    conn.busy_timeout(Duration::from_secs(30))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    conn.execute_batch("
        CREATE TABLE IF NOT EXISTS contour_tile_manifest (
            zoom_bucket INTEGER NOT NULL,
            lat_bucket  INTEGER NOT NULL,
            lon_bucket  INTEGER NOT NULL,
            contour_count INTEGER NOT NULL,
            built_at    INTEGER NOT NULL DEFAULT (unixepoch()),
            PRIMARY KEY (zoom_bucket, lat_bucket, lon_bucket)
        );
        CREATE TABLE IF NOT EXISTS contour_tiles (
            zoom_bucket INTEGER NOT NULL,
            lat_bucket  INTEGER NOT NULL,
            lon_bucket  INTEGER NOT NULL,
            fid         INTEGER NOT NULL,
            elevation_m REAL    NOT NULL,
            geom        BLOB    NOT NULL,
            PRIMARY KEY (zoom_bucket, lat_bucket, lon_bucket, fid)
        );
        CREATE INDEX IF NOT EXISTS idx_contour_tiles_lookup
            ON contour_tiles (zoom_bucket, lat_bucket, lon_bucket, elevation_m, fid);
        CREATE TABLE IF NOT EXISTS coastline_tile_manifest (
            zoom_bucket INTEGER NOT NULL,
            lat_bucket  INTEGER NOT NULL,
            lon_bucket  INTEGER NOT NULL,
            line_count  INTEGER NOT NULL,
            built_at    INTEGER NOT NULL DEFAULT (unixepoch()),
            PRIMARY KEY (zoom_bucket, lat_bucket, lon_bucket)
        );
        CREATE TABLE IF NOT EXISTS coastline_tiles (
            zoom_bucket INTEGER NOT NULL,
            lat_bucket  INTEGER NOT NULL,
            lon_bucket  INTEGER NOT NULL,
            fid         INTEGER NOT NULL,
            geom        BLOB    NOT NULL,
            PRIMARY KEY (zoom_bucket, lat_bucket, lon_bucket, fid)
        );
        CREATE INDEX IF NOT EXISTS idx_coastline_tiles_lookup
            ON coastline_tiles (zoom_bucket, lat_bucket, lon_bucket, fid);
    ")?;
    Ok(conn)
}

fn tile_exists(conn: &Connection, tile: TileKey) -> bool {
    conn.query_row(
        "SELECT 1 FROM contour_tile_manifest
         WHERE zoom_bucket=?1 AND lat_bucket=?2 AND lon_bucket=?3 LIMIT 1",
        params![tile.zoom_bucket, tile.lat_bucket, tile.lon_bucket],
        |_| Ok(()),
    )
    .optional()
    .map(|v| v.is_some())
    .unwrap_or(false)
}

fn import_tile(cache_db_path: &Path, tile: TileKey, gpkg_path: &Path) -> rusqlite::Result<()> {
    let source = Connection::open(gpkg_path)?;
    source.busy_timeout(Duration::from_secs(30))?;
    let mut stmt = source
        .prepare("SELECT fid, geom, elevation_m FROM contour ORDER BY ABS(elevation_m), fid")?;
    let mut rows = stmt.query([])?;

    let mut cache = open_cache_db(cache_db_path)?;
    let tx = cache.transaction()?;
    tx.execute(
        "DELETE FROM contour_tiles WHERE zoom_bucket=?1 AND lat_bucket=?2 AND lon_bucket=?3",
        params![tile.zoom_bucket, tile.lat_bucket, tile.lon_bucket],
    )?;
    tx.execute(
        "DELETE FROM contour_tile_manifest WHERE zoom_bucket=?1 AND lat_bucket=?2 AND lon_bucket=?3",
        params![tile.zoom_bucket, tile.lat_bucket, tile.lon_bucket],
    )?;

    let mut count = 0usize;
    while let Some(row) = rows.next()? {
        let fid: i64 = row.get(0)?;
        let geom: Vec<u8> = row.get(1)?;
        let elev: f32 = row.get(2)?;
        tx.execute(
            "INSERT INTO contour_tiles (zoom_bucket,lat_bucket,lon_bucket,fid,elevation_m,geom)
             VALUES (?1,?2,?3,?4,?5,?6)",
            params![tile.zoom_bucket, tile.lat_bucket, tile.lon_bucket, fid, elev, geom],
        )?;
        count += 1;
    }

    tx.execute(
        "INSERT INTO contour_tile_manifest (zoom_bucket,lat_bucket,lon_bucket,contour_count,built_at)
         VALUES (?1,?2,?3,?4,unixepoch())",
        params![tile.zoom_bucket, tile.lat_bucket, tile.lon_bucket, count as i64],
    )?;
    tx.commit()
}

fn import_coastline(cache_db_path: &Path, tile: TileKey, gpkg_path: &Path) -> rusqlite::Result<()> {
    let source = Connection::open(gpkg_path)?;
    source.busy_timeout(Duration::from_secs(30))?;

    let table_exists: bool = source
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='contour'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map(|n| n > 0)
        .unwrap_or(false);

    let mut cache = open_cache_db(cache_db_path)?;
    let tx = cache.transaction()?;
    tx.execute(
        "DELETE FROM coastline_tiles WHERE zoom_bucket=?1 AND lat_bucket=?2 AND lon_bucket=?3",
        params![tile.zoom_bucket, tile.lat_bucket, tile.lon_bucket],
    )?;
    tx.execute(
        "DELETE FROM coastline_tile_manifest WHERE zoom_bucket=?1 AND lat_bucket=?2 AND lon_bucket=?3",
        params![tile.zoom_bucket, tile.lat_bucket, tile.lon_bucket],
    )?;

    let mut count = 0usize;
    if table_exists {
        let mut stmt = source.prepare("SELECT fid, geom FROM contour ORDER BY fid")?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let fid: i64 = row.get(0)?;
            let geom: Vec<u8> = row.get(1)?;
            tx.execute(
                "INSERT INTO coastline_tiles (zoom_bucket,lat_bucket,lon_bucket,fid,geom)
                 VALUES (?1,?2,?3,?4,?5)",
                params![tile.zoom_bucket, tile.lat_bucket, tile.lon_bucket, fid, geom],
            )?;
            count += 1;
        }
    }

    tx.execute(
        "INSERT INTO coastline_tile_manifest (zoom_bucket,lat_bucket,lon_bucket,line_count)
         VALUES (?1,?2,?3,?4)",
        params![tile.zoom_bucket, tile.lat_bucket, tile.lon_bucket, count as i64],
    )?;
    tx.commit()
}

// ── GDAL helpers (mirrors desktop gdal.rs) ────────────────────────────────────

fn run_gdal(mut cmd: Command, label: &str) -> std::io::Result<()> {
    let start = Instant::now();
    let timeout = Duration::from_secs(120);
    let mut child = cmd.spawn()?;
    loop {
        if let Some(status) = child.try_wait()? {
            return if status.success() {
                Ok(())
            } else {
                Err(std::io::Error::other(format!("{label} failed with status {status}")))
            };
        }
        if start.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("{label} timed out"),
            ));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn srtm_tile_paths(srtm_root: &Path, bounds: GeoBounds) -> Vec<PathBuf> {
    let mut tiles = Vec::new();
    for lat in (bounds.min_lat.floor() as i32)..=(bounds.max_lat.floor() as i32) {
        for lon in (bounds.min_lon.floor() as i32)..=(bounds.max_lon.floor() as i32) {
            let ns = if lat >= 0 { 'N' } else { 'S' };
            let ew = if lon >= 0 { 'E' } else { 'W' };
            let name = format!("{}{:02}{}{:03}.tif", ns, lat.unsigned_abs(), ew, lon.unsigned_abs());
            let path = srtm_root.join(&name);
            if path.exists() {
                tiles.push(path);
            }
        }
    }
    tiles
}

fn run_gdalwarp(
    gdal_bin: &Path,
    tiles: &[PathBuf],
    out_tif: &Path,
    bounds: GeoBounds,
    spec: FocusContourSpec,
) -> std::io::Result<()> {
    let mut cmd = Command::new(gdal_bin.join("gdalwarp"));
    cmd.args([
        "-q", "-overwrite", "-r", "bilinear",
        "-dstnodata", "-32768",
        "-te",
        &format!("{:.6}", bounds.min_lon),
        &format!("{:.6}", bounds.min_lat),
        &format!("{:.6}", bounds.max_lon),
        &format!("{:.6}", bounds.max_lat),
        "-ts", &spec.raster_size.to_string(), &spec.raster_size.to_string(),
    ]);
    for tile in tiles { cmd.arg(tile); }
    cmd.arg(out_tif);
    run_gdal(cmd, "gdalwarp")
}

fn run_gdal_contour(
    gdal_bin: &Path,
    in_tif: &Path,
    out_gpkg: &Path,
    interval_m: i32,
) -> std::io::Result<()> {
    let mut cmd = Command::new(gdal_bin.join("gdal_contour"));
    cmd.args(["-q", "-f", "GPKG", "-a", "elevation_m",
              "-i", &interval_m.to_string(),
              "-snodata", "-32768",
              "-nln", "contour"]);
    cmd.arg(in_tif);
    cmd.arg(out_gpkg);
    run_gdal(cmd, "gdal_contour")
}

fn run_gdal_coastline(gdal_bin: &Path, in_tif: &Path, out_gpkg: &Path) -> std::io::Result<()> {
    let mut cmd = Command::new(gdal_bin.join("gdal_contour"));
    cmd.args(["-q", "-f", "GPKG", "-a", "elevation_m",
              "-fl", "0", "-snodata", "-32768", "-nln", "contour"]);
    cmd.arg(in_tif);
    cmd.arg(out_gpkg);
    run_gdal(cmd, "gdal_contour (coastline 0m)")
}

fn cleanup(paths: &[&Path]) {
    for p in paths { let _ = fs::remove_file(p); }
}

// ── Public entry point ────────────────────────────────────────────────────────

pub struct ContourBuildProgress {
    pub stage:   String,
    pub fraction: f32,
    pub message: String,
}

/// Pre-build contour tiles for every zoom bucket in `zoom_buckets` that covers
/// the given bounding box, writing into `cache_db_path` (which the desktop
/// reads as `Derived/terrain/srtm_focus_cache.sqlite`).
///
/// `gdal_bin_dir` is the directory containing `gdalwarp` / `gdal_contour`.
/// Pass `Path::new("")` to use the system PATH.
pub fn build_contour_tiles(
    srtm_root: &Path,
    cache_db_path: &Path,
    tmp_dir: &Path,
    bounds: GeoBounds,
    zoom_buckets: &[i32],
    gdal_bin_dir: &Path,
    progress: &mut dyn FnMut(ContourBuildProgress),
) -> Result<String, String> {
    fs::create_dir_all(tmp_dir).map_err(|e| e.to_string())?;

    // Ensure schema exists before we start
    open_cache_db(cache_db_path).map_err(|e| e.to_string())?;

    let specs = all_specs();
    let selected: Vec<FocusContourSpec> = specs
        .iter()
        .filter(|s| zoom_buckets.contains(&s.zoom_bucket))
        .copied()
        .collect();

    // Count total tiles up front for progress reporting
    let total_tiles: usize = selected.iter().map(|spec| {
        let bucket_step = spec.half_extent_deg * 0.45;
        let lat_buckets = tiles_in_range(bounds.min_lat, bounds.max_lat, bucket_step);
        let lon_buckets = tiles_in_range(bounds.min_lon, bounds.max_lon, bucket_step);
        lat_buckets * lon_buckets
    }).sum();

    let mut done = 0usize;
    let mut built = 0usize;
    let mut skipped = 0usize;

    let conn = open_cache_db(cache_db_path).map_err(|e| e.to_string())?;

    for spec in &selected {
        let bucket_step = spec.half_extent_deg * 0.45;
        let lat_range = bucket_range(bounds.min_lat, bounds.max_lat, bucket_step);
        let lon_range = bucket_range(bounds.min_lon, bounds.max_lon, bucket_step);

        for lat_bucket in lat_range.clone() {
            for lon_bucket in lon_range.clone() {
                let tile = TileKey { zoom_bucket: spec.zoom_bucket, lat_bucket, lon_bucket };

                progress(ContourBuildProgress {
                    stage:   format!("Zoom bucket {}", spec.zoom_bucket),
                    fraction: done as f32 / total_tiles.max(1) as f32,
                    message: format!(
                        "z{} tile ({lat_bucket},{lon_bucket}) — {done}/{total_tiles}",
                        spec.zoom_bucket
                    ),
                });

                if tile_exists(&conn, tile) {
                    skipped += 1;
                    done += 1;
                    continue;
                }

                let bucket_center_lat = (lat_bucket as f32 * bucket_step).clamp(-89.999, 89.999);
                let bucket_center_lon = lon_bucket as f32 * bucket_step;
                let tile_bounds = GeoBounds {
                    min_lat: (bucket_center_lat - spec.half_extent_deg).clamp(-89.999, 89.999),
                    max_lat: (bucket_center_lat + spec.half_extent_deg).clamp(-89.999, 89.999),
                    min_lon: bucket_center_lon - spec.half_extent_deg,
                    max_lon: bucket_center_lon + spec.half_extent_deg,
                };

                let srtm_tiles = srtm_tile_paths(srtm_root, tile_bounds);
                if srtm_tiles.is_empty() {
                    done += 1;
                    continue; // no SRTM coverage — skip silently
                }

                let stem = format!("z{}_lat{}_lon{}", tile.zoom_bucket, lat_bucket, lon_bucket);
                let tmp_tif  = tmp_dir.join(format!("{stem}.tmp.tif"));
                let tmp_gpkg = tmp_dir.join(format!("{stem}.tmp.gpkg"));
                let tmp_coast_gpkg = tmp_dir.join(format!("{stem}.coast.tmp.gpkg"));

                cleanup(&[&tmp_tif, &tmp_gpkg, &tmp_coast_gpkg]);

                if let Err(e) = run_gdalwarp(gdal_bin_dir, &srtm_tiles, &tmp_tif, tile_bounds, *spec) {
                    cleanup(&[&tmp_tif]);
                    progress(ContourBuildProgress {
                        stage: format!("Zoom bucket {}", spec.zoom_bucket),
                        fraction: done as f32 / total_tiles.max(1) as f32,
                        message: format!("gdalwarp failed for z{} ({lat_bucket},{lon_bucket}): {e}", spec.zoom_bucket),
                    });
                    done += 1;
                    continue;
                }

                if let Err(e) = run_gdal_contour(gdal_bin_dir, &tmp_tif, &tmp_gpkg, spec.interval_m) {
                    cleanup(&[&tmp_tif, &tmp_gpkg]);
                    progress(ContourBuildProgress {
                        stage: format!("Zoom bucket {}", spec.zoom_bucket),
                        fraction: done as f32 / total_tiles.max(1) as f32,
                        message: format!("gdal_contour failed for z{} ({lat_bucket},{lon_bucket}): {e}", spec.zoom_bucket),
                    });
                    done += 1;
                    continue;
                }

                if let Err(e) = import_tile(cache_db_path, tile, &tmp_gpkg) {
                    cleanup(&[&tmp_tif, &tmp_gpkg]);
                    progress(ContourBuildProgress {
                        stage: format!("Zoom bucket {}", spec.zoom_bucket),
                        fraction: done as f32 / total_tiles.max(1) as f32,
                        message: format!("DB import failed for z{} ({lat_bucket},{lon_bucket}): {e}", spec.zoom_bucket),
                    });
                    done += 1;
                    continue;
                }

                // Piggyback coastline 0m extraction (matches desktop behaviour)
                if run_gdal_coastline(gdal_bin_dir, &tmp_tif, &tmp_coast_gpkg).is_ok() {
                    let _ = import_coastline(cache_db_path, tile, &tmp_coast_gpkg);
                }

                cleanup(&[&tmp_tif, &tmp_gpkg, &tmp_coast_gpkg]);
                built += 1;
                done += 1;
            }
        }
    }

    Ok(format!(
        "Contours complete: {built} tiles built, {skipped} already cached, {} total",
        done
    ))
}

// ── Tile range helpers ────────────────────────────────────────────────────────

fn bucket_range(coord_min: f32, coord_max: f32, step: f32) -> std::ops::RangeInclusive<i32> {
    let lo = (coord_min / step).floor() as i32;
    let hi = (coord_max / step).ceil() as i32;
    lo..=hi
}

fn tiles_in_range(coord_min: f32, coord_max: f32, step: f32) -> usize {
    let lo = (coord_min / step).floor() as i32;
    let hi = (coord_max / step).ceil() as i32;
    (hi - lo + 1).max(0) as usize
}
