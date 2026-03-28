/// Offline contour pre-builder — Phase A (GDAL pipeline, SQLite output).
///
/// Iterates every tile in `zoom_buckets` that covers the requested bounding
/// box, skips tiles already present in `srtm_focus_cache.sqlite`, and runs
/// the same gdalwarp + gdal_contour pipeline the desktop uses on-demand.
/// The desktop will find the pre-built tiles and skip its own generation.
use rayon::prelude::*;
use rusqlite::{Connection, OptionalExtension, params};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
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
    gdalwarp: &Path,
    tiles: &[PathBuf],
    out_tif: &Path,
    bounds: GeoBounds,
    spec: FocusContourSpec,
) -> std::io::Result<()> {
    let mut cmd = Command::new(gdalwarp);
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
    gdal_contour: &Path,
    in_tif: &Path,
    out_gpkg: &Path,
    interval_m: i32,
) -> std::io::Result<()> {
    let mut cmd = Command::new(gdal_contour);
    cmd.args(["-q", "-f", "GPKG", "-a", "elevation_m",
              "-i", &interval_m.to_string(),
              "-snodata", "-32768",
              "-nln", "contour"]);
    cmd.arg(in_tif);
    cmd.arg(out_gpkg);
    run_gdal(cmd, "gdal_contour")
}

fn run_gdal_coastline(gdal_contour: &Path, in_tif: &Path, out_gpkg: &Path) -> std::io::Result<()> {
    let mut cmd = Command::new(gdal_contour);
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
    pub stage:    String,
    pub fraction: f32,
    pub message:  String,
    pub is_error: bool,
    /// Set when a tile was just completed; carries (min_lat, max_lat, min_lon, max_lon).
    pub tile_bounds: Option<(f32, f32, f32, f32)>,
}

impl ContourBuildProgress {
    fn info(stage: impl Into<String>, fraction: f32, message: impl Into<String>) -> Self {
        Self { stage: stage.into(), fraction, message: message.into(), is_error: false, tile_bounds: None }
    }
    fn error(stage: impl Into<String>, fraction: f32, message: impl Into<String>) -> Self {
        Self { stage: stage.into(), fraction, message: message.into(), is_error: true, tile_bounds: None }
    }
    fn built(stage: impl Into<String>, fraction: f32, message: impl Into<String>, bounds: (f32, f32, f32, f32)) -> Self {
        Self { stage: stage.into(), fraction, message: message.into(), is_error: false, tile_bounds: Some(bounds) }
    }
}

/// Resolve the full path to a GDAL tool, checking the explicit bin dir first
/// then common Homebrew locations, then falling back to bare name (PATH).
fn resolve_gdal_tool(gdal_bin_dir: &Path, tool: &str) -> PathBuf {
    // 1. Explicit bin dir from the user
    if gdal_bin_dir != Path::new("") {
        let candidate = gdal_bin_dir.join(tool);
        if candidate.exists() {
            return candidate;
        }
    }
    // 2. Homebrew (Apple Silicon then Intel)
    for prefix in &["/opt/homebrew/bin", "/usr/local/bin"] {
        let candidate = PathBuf::from(prefix).join(tool);
        if candidate.exists() {
            return candidate;
        }
    }
    // 3. Bare name — relies on PATH (works in CLI context)
    PathBuf::from(tool)
}

/// Pre-build contour tiles for every zoom bucket in `zoom_buckets` that covers
/// the given bounding box, writing into `cache_db_path` (which the desktop
/// reads as `Derived/terrain/srtm_focus_cache.sqlite`).
///
/// `gdal_bin_dir` is the directory containing `gdalwarp` / `gdal_contour`.
/// Pass `Path::new("")` to search Homebrew locations then `$PATH`.
pub fn build_contour_tiles(
    srtm_root: &Path,
    cache_db_path: &Path,
    tmp_dir: &Path,
    bounds: GeoBounds,
    zoom_buckets: &[i32],
    gdal_bin_dir: &Path,
    progress: &mut dyn FnMut(ContourBuildProgress),
) -> Result<String, String> {
    // ── Validate SRTM root ────────────────────────────────────────────────────
    if !srtm_root.exists() {
        return Err(format!("SRTM root does not exist: {}", srtm_root.display()));
    }
    let srtm_tile_count = fs::read_dir(srtm_root)
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("tif"))
                .count()
        })
        .unwrap_or(0);
    if srtm_tile_count == 0 {
        return Err(format!(
            "No .tif files found directly in SRTM root: {}. \
             Make sure the directory contains files like N37W122.tif.",
            srtm_root.display()
        ));
    }
    progress(ContourBuildProgress::info(
        "Startup",
        0.0,
        format!("SRTM root OK — {srtm_tile_count} tiles found"),
    ));

    // ── Validate GDAL ─────────────────────────────────────────────────────────
    let gdalwarp = resolve_gdal_tool(gdal_bin_dir, "gdalwarp");
    let gdal_contour = resolve_gdal_tool(gdal_bin_dir, "gdal_contour");
    for (tool_path, name) in [(&gdalwarp, "gdalwarp"), (&gdal_contour, "gdal_contour")] {
        match std::process::Command::new(tool_path).arg("--version").output() {
            Ok(out) if out.status.success() => {
                let version = String::from_utf8_lossy(&out.stdout);
                progress(ContourBuildProgress::info(
                    "Startup",
                    0.0,
                    format!("{name}: {}", version.trim()),
                ));
            }
            Ok(_) => {
                return Err(format!("{name} at '{}' returned an error on --version", tool_path.display()));
            }
            Err(e) => {
                return Err(format!(
                    "Could not launch {name} at '{}': {e}. \
                     Set GDAL bin dir to the folder containing gdalwarp.",
                    tool_path.display()
                ));
            }
        }
    }

    fs::create_dir_all(tmp_dir).map_err(|e| e.to_string())?;
    open_cache_db(cache_db_path).map_err(|e| e.to_string())?;

    let specs = all_specs();
    let selected: Vec<FocusContourSpec> = specs
        .iter()
        .filter(|s| zoom_buckets.contains(&s.zoom_bucket))
        .copied()
        .collect();

    // ── Phase 1: collect work (single-threaded, needs SQLite connection) ──────
    progress(ContourBuildProgress::info("Planning", 0.0, "Scanning tiles…"));

    struct TileWork {
        tile: TileKey,
        tile_bounds: GeoBounds,
        spec: FocusContourSpec,
        srtm_tiles: Vec<PathBuf>,
    }

    let conn = open_cache_db(cache_db_path).map_err(|e| e.to_string())?;
    let mut work: Vec<TileWork> = Vec::new();
    let mut skipped = 0usize;

    for spec in &selected {
        let bucket_step = spec.half_extent_deg * 0.45;
        for lat_bucket in bucket_range(bounds.min_lat, bounds.max_lat, bucket_step) {
            for lon_bucket in bucket_range(bounds.min_lon, bounds.max_lon, bucket_step) {
                let tile = TileKey { zoom_bucket: spec.zoom_bucket, lat_bucket, lon_bucket };
                if tile_exists(&conn, tile) {
                    skipped += 1;
                    continue;
                }
                let center_lat = (lat_bucket as f32 * bucket_step).clamp(-89.999, 89.999);
                let center_lon = lon_bucket as f32 * bucket_step;
                let tile_bounds = GeoBounds {
                    min_lat: (center_lat - spec.half_extent_deg).clamp(-89.999, 89.999),
                    max_lat: (center_lat + spec.half_extent_deg).clamp(-89.999, 89.999),
                    min_lon: center_lon - spec.half_extent_deg,
                    max_lon: center_lon + spec.half_extent_deg,
                };
                let srtm_tiles = srtm_tile_paths(srtm_root, tile_bounds);
                if !srtm_tiles.is_empty() {
                    work.push(TileWork { tile, tile_bounds, spec: *spec, srtm_tiles });
                }
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

    if work.is_empty() {
        return Ok(format!("Contours complete: 0 built, {skipped} already cached, {total} total"));
    }

    // ── Phase 2: parallel build ───────────────────────────────────────────────
    let num_threads = (std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        / 2)
        .max(2)
        .min(8);
    progress(ContourBuildProgress::info(
        "Building",
        0.0,
        format!("Starting {num_threads} parallel workers"),
    ));

    enum Outcome { Built(f32, f32, f32, f32), Error(String) }
    let (tx, rx) = mpsc::channel::<Outcome>();
    let done_count = Arc::new(AtomicUsize::new(0));

    // Capture by-value for the spawned thread
    let gdalwarp = gdalwarp.clone();
    let gdal_contour = gdal_contour.clone();
    let cache_db_path = cache_db_path.to_path_buf();
    let tmp_dir = tmp_dir.to_path_buf();
    let done_arc = done_count.clone();

    std::thread::spawn(move || {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(num_threads)
            .build()
            .expect("rayon pool");
        pool.install(|| {
            work.into_par_iter().for_each_with(tx, |tx, w| {
                let TileWork { tile, tile_bounds, spec, srtm_tiles } = w;
                let stem = format!(
                    "z{}_lat{}_lon{}",
                    tile.zoom_bucket, tile.lat_bucket, tile.lon_bucket
                );
                let tmp_tif        = tmp_dir.join(format!("{stem}.tmp.tif"));
                let tmp_gpkg       = tmp_dir.join(format!("{stem}.tmp.gpkg"));
                let tmp_coast_gpkg = tmp_dir.join(format!("{stem}.coast.tmp.gpkg"));
                cleanup(&[&tmp_tif, &tmp_gpkg, &tmp_coast_gpkg]);

                let outcome = (|| {
                    run_gdalwarp(&gdalwarp, &srtm_tiles, &tmp_tif, tile_bounds, spec)
                        .map_err(|e| format!("gdalwarp: {e}"))?;
                    run_gdal_contour(&gdal_contour, &tmp_tif, &tmp_gpkg, spec.interval_m)
                        .map_err(|e| format!("gdal_contour: {e}"))?;
                    import_tile(&cache_db_path, tile, &tmp_gpkg)
                        .map_err(|e| format!("db import: {e}"))?;
                    if run_gdal_coastline(&gdal_contour, &tmp_tif, &tmp_coast_gpkg).is_ok() {
                        let _ = import_coastline(&cache_db_path, tile, &tmp_coast_gpkg);
                    }
                    Ok::<_, String>(())
                })();

                cleanup(&[&tmp_tif, &tmp_gpkg, &tmp_coast_gpkg]);
                done_arc.fetch_add(1, Ordering::Relaxed);
                let _ = tx.send(match outcome {
                    Ok(()) => Outcome::Built(
                        tile_bounds.min_lat, tile_bounds.max_lat,
                        tile_bounds.min_lon, tile_bounds.max_lon,
                    ),
                    Err(e) => Outcome::Error(format!(
                        "z{} ({},{}) — {e}",
                        tile.zoom_bucket, tile.lat_bucket, tile.lon_bucket
                    )),
                });
            });
        });
    });

    // ── Phase 3: drain results, report progress ───────────────────────────────
    let mut built = 0usize;
    let mut errors = 0usize;
    for outcome in rx {
        let done = done_count.load(Ordering::Relaxed);
        let frac = done as f32 / to_build.max(1) as f32;
        match outcome {
            Outcome::Built(min_lat, max_lat, min_lon, max_lon) => {
                built += 1;
                progress(ContourBuildProgress::built(
                    "Building",
                    frac,
                    format!("{done}/{to_build} tiles built"),
                    (min_lat, max_lat, min_lon, max_lon),
                ));
            }
            Outcome::Error(msg) => {
                errors += 1;
                progress(ContourBuildProgress::error("Building", frac, msg));
            }
        }
    }

    Ok(format!(
        "Contours complete: {built} built, {skipped} cached, {errors} errors, {total} total"
    ))
}

// ── Native (marching-squares) builder ────────────────────────────────────────

/// Encode a slice of (lon, lat) f32 pairs as a GeoPackage geometry blob
/// containing a WKB LineString (EPSG:4326, little-endian, no envelope).
pub fn encode_gpkg_linestring(points: &[(f32, f32)]) -> Vec<u8> {
    let n = points.len() as u32;
    // 8-byte GPKG header + 1+4+4 WKB header + n*16 point bytes
    let mut buf = Vec::with_capacity(8 + 9 + n as usize * 16);
    // GPKG header
    buf.extend_from_slice(b"GP");
    buf.push(0x00); // version
    buf.push(0x00); // flags: LE WKB, no envelope
    buf.extend_from_slice(&4326u32.to_le_bytes()); // SRS ID
    // WKB LineString
    buf.push(0x01); // little-endian
    buf.extend_from_slice(&2u32.to_le_bytes()); // type = LineString
    buf.extend_from_slice(&n.to_le_bytes());
    for &(lon, lat) in points {
        buf.extend_from_slice(&(lon as f64).to_le_bytes());
        buf.extend_from_slice(&(lat as f64).to_le_bytes());
    }
    buf
}

/// Write one tile's contours and coastlines in a single transaction.
/// Takes a persistent `&mut Connection` — the caller holds it open across all tiles,
/// avoiding the per-tile open/pragma/DDL overhead of opening a new connection each time.
fn write_tile_native(
    conn: &mut Connection,
    tile: TileKey,
    contours: &[crate::marching_squares::ContourLine],
    coastlines: &[Vec<(f32, f32)>],
) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;

    // ── Contours ──────────────────────────────────────────────────────────────
    tx.execute(
        "DELETE FROM contour_tiles WHERE zoom_bucket=?1 AND lat_bucket=?2 AND lon_bucket=?3",
        params![tile.zoom_bucket, tile.lat_bucket, tile.lon_bucket],
    )?;
    tx.execute(
        "DELETE FROM contour_tile_manifest WHERE zoom_bucket=?1 AND lat_bucket=?2 AND lon_bucket=?3",
        params![tile.zoom_bucket, tile.lat_bucket, tile.lon_bucket],
    )?;
    let mut contour_count = 0usize;
    for (fid, line) in contours.iter().enumerate() {
        let blob = encode_gpkg_linestring(&line.points);
        tx.execute(
            "INSERT INTO contour_tiles (zoom_bucket,lat_bucket,lon_bucket,fid,elevation_m,geom)
             VALUES (?1,?2,?3,?4,?5,?6)",
            params![tile.zoom_bucket, tile.lat_bucket, tile.lon_bucket,
                    fid as i64, line.elevation_m, blob],
        )?;
        contour_count += 1;
    }
    tx.execute(
        "INSERT INTO contour_tile_manifest (zoom_bucket,lat_bucket,lon_bucket,contour_count,built_at)
         VALUES (?1,?2,?3,?4,unixepoch())",
        params![tile.zoom_bucket, tile.lat_bucket, tile.lon_bucket, contour_count as i64],
    )?;

    // ── Coastlines ────────────────────────────────────────────────────────────
    tx.execute(
        "DELETE FROM coastline_tiles WHERE zoom_bucket=?1 AND lat_bucket=?2 AND lon_bucket=?3",
        params![tile.zoom_bucket, tile.lat_bucket, tile.lon_bucket],
    )?;
    tx.execute(
        "DELETE FROM coastline_tile_manifest WHERE zoom_bucket=?1 AND lat_bucket=?2 AND lon_bucket=?3",
        params![tile.zoom_bucket, tile.lat_bucket, tile.lon_bucket],
    )?;
    let mut coastline_count = 0usize;
    for (fid, pts) in coastlines.iter().enumerate() {
        let blob = encode_gpkg_linestring(pts);
        tx.execute(
            "INSERT INTO coastline_tiles (zoom_bucket,lat_bucket,lon_bucket,fid,geom)
             VALUES (?1,?2,?3,?4,?5)",
            params![tile.zoom_bucket, tile.lat_bucket, tile.lon_bucket, fid as i64, blob],
        )?;
        coastline_count += 1;
    }
    tx.execute(
        "INSERT INTO coastline_tile_manifest (zoom_bucket,lat_bucket,lon_bucket,line_count)
         VALUES (?1,?2,?3,?4)",
        params![tile.zoom_bucket, tile.lat_bucket, tile.lon_bucket, coastline_count as i64],
    )?;

    tx.commit()
}

/// Pre-build contour tiles using pure-Rust marching squares — no GDAL required.
///
/// Same parameters and output schema as `build_contour_tiles`, but reads SRTM
/// tiles in-process with bilinear interpolation and extracts iso-lines via
/// marching squares.  `gdal_bin_dir` is ignored (kept for API consistency).
pub fn build_contour_tiles_native(
    srtm_root: &Path,
    cache_db_path: &Path,
    bounds: GeoBounds,
    zoom_buckets: &[i32],
    progress: &mut dyn FnMut(ContourBuildProgress),
) -> Result<String, String> {
    use crate::marching_squares::{NativeSrtmSampler, build_tile_contours};

    // ── Validate SRTM root ────────────────────────────────────────────────────
    if !srtm_root.exists() {
        return Err(format!("SRTM root does not exist: {}", srtm_root.display()));
    }
    let srtm_tile_count = fs::read_dir(srtm_root)
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("tif"))
                .count()
        })
        .unwrap_or(0);
    if srtm_tile_count == 0 {
        return Err(format!(
            "No .tif files found in SRTM root: {}",
            srtm_root.display()
        ));
    }
    progress(ContourBuildProgress::info(
        "Startup",
        0.0,
        format!("Native engine — {srtm_tile_count} SRTM tiles found"),
    ));
    progress(ContourBuildProgress::info(
        "Startup",
        0.0,
        format!("Cache DB: {}", cache_db_path.display()),
    ));

    open_cache_db(cache_db_path).map_err(|e| e.to_string())?;

    let specs = all_specs();
    let selected: Vec<FocusContourSpec> = specs
        .iter()
        .filter(|s| zoom_buckets.contains(&s.zoom_bucket))
        .copied()
        .collect();

    // ── Planning phase ────────────────────────────────────────────────────────
    progress(ContourBuildProgress::info("Planning", 0.0, "Scanning tiles…"));

    let conn = open_cache_db(cache_db_path).map_err(|e| e.to_string())?;
    let mut work: Vec<(TileKey, GeoBounds, FocusContourSpec)> = Vec::new();
    let mut skipped = 0usize;

    for spec in &selected {
        let bucket_step = spec.half_extent_deg * 0.45;
        for lat_bucket in bucket_range(bounds.min_lat, bounds.max_lat, bucket_step) {
            for lon_bucket in bucket_range(bounds.min_lon, bounds.max_lon, bucket_step) {
                let tile = TileKey { zoom_bucket: spec.zoom_bucket, lat_bucket, lon_bucket };
                if tile_exists(&conn, tile) {
                    skipped += 1;
                    continue;
                }
                let center_lat = (lat_bucket as f32 * bucket_step).clamp(-89.999, 89.999);
                let center_lon = lon_bucket as f32 * bucket_step;
                let tile_bounds = GeoBounds {
                    min_lat: (center_lat - spec.half_extent_deg).clamp(-89.999, 89.999),
                    max_lat: (center_lat + spec.half_extent_deg).clamp(-89.999, 89.999),
                    min_lon: center_lon - spec.half_extent_deg,
                    max_lon: center_lon + spec.half_extent_deg,
                };
                // Only plan tiles that have at least one SRTM tile in range.
                if !srtm_tile_paths(srtm_root, tile_bounds).is_empty() {
                    work.push((tile, tile_bounds, *spec));
                }
            }
        }
    }
    drop(conn);

    // Interleave zoom levels so all levels progress simultaneously — avoids the
    // visual artifact where low-zoom (large) tiles complete first and flood the
    // map with solid blocks before high-zoom tiles even start.
    // Round-robin: take tile[0] from z0, tile[0] from z1, …, tile[1] from z0, …
    {
        let mut by_zoom: std::collections::BTreeMap<i32, Vec<_>> = std::collections::BTreeMap::new();
        for item in work.drain(..) {
            by_zoom.entry(item.0.zoom_bucket).or_default().push(item);
        }
        let buckets: Vec<Vec<_>> = by_zoom.into_values().collect();
        let max_len = buckets.iter().map(|v| v.len()).max().unwrap_or(0);
        for i in 0..max_len {
            for bucket in &buckets {
                if i < bucket.len() {
                    work.push(bucket[i]);
                }
            }
        }
    }

    let total = work.len() + skipped;
    let to_build = work.len();
    // Log path + skip count prominently so mismatched cache paths are obvious
    progress(ContourBuildProgress::info(
        "Planning",
        0.0,
        format!(
            "Plan: {to_build} to build, {skipped} skipped (already cached), {total} in bbox — DB: {}",
            cache_db_path.display()
        ),
    ));

    if work.is_empty() {
        return Ok(format!("All {total} tiles already cached — nothing to build"));
    }

    // ── Build phase ───────────────────────────────────────────────────────────
    // Architecture: N rayon compute workers → mpsc channel → 1 writer thread
    //
    // Rayon workers only do CPU work (marching squares + SRTM I/O).  A single
    // dedicated writer thread drains results and writes to SQLite with zero
    // lock contention.  This lets all cores run at full speed.
    let num_threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    progress(ContourBuildProgress::info(
        "Building",
        0.0,
        format!("Starting {num_threads} compute workers + 1 writer"),
    ));

    // compute workers → writer thread
    type ComputeResult = (TileKey, GeoBounds, Vec<crate::marching_squares::ContourLine>, Vec<Vec<(f32, f32)>>);
    let (compute_tx, compute_rx) = mpsc::channel::<ComputeResult>();

    // writer thread → main thread (outcome events)
    enum Outcome { Built(f32, f32, f32, f32), Error(String) }
    let (outcome_tx, rx) = mpsc::channel::<Outcome>();
    let done_count = Arc::new(AtomicUsize::new(0));

    let srtm_root_owned = srtm_root.to_path_buf();

    // ── Compute thread: fans work out to rayon, sends raw results ────────────
    std::thread::spawn(move || {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(num_threads)
            .build()
            .expect("rayon pool");
        pool.install(|| {
            work.into_par_iter().for_each_with(compute_tx, |tx, (tile, tile_bounds, spec)| {
                let mut sampler = NativeSrtmSampler::new(srtm_root_owned.clone());
                let (contours, coastlines) =
                    build_tile_contours(&mut sampler, spec, tile_bounds);
                let _ = tx.send((tile, tile_bounds, contours, coastlines));
            });
        });
        // compute_tx drops here, closing the channel → writer thread exits
    });

    // ── Writer thread: single SQLite writer, no contention ───────────────────
    // One persistent connection for all tiles — avoids the per-tile
    // open+pragma+DDL overhead (~2 ms each × 155k tiles = 5+ minutes wasted).
    let cache_db_owned = cache_db_path.to_path_buf();
    let done_arc = done_count.clone();
    std::thread::spawn(move || {
        let mut conn = match open_cache_db(&cache_db_owned) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[1kEE] writer thread: failed to open cache DB: {e}");
                return;
            }
        };
        for (tile, tile_bounds, contours, coastlines) in compute_rx {
            let outcome = write_tile_native(&mut conn, tile, &contours, &coastlines);
            done_arc.fetch_add(1, Ordering::Relaxed);
            let _ = outcome_tx.send(match outcome {
                Ok(()) => Outcome::Built(
                    tile_bounds.min_lat, tile_bounds.max_lat,
                    tile_bounds.min_lon, tile_bounds.max_lon,
                ),
                Err(e) => Outcome::Error(format!(
                    "z{} ({},{}) — {e}",
                    tile.zoom_bucket, tile.lat_bucket, tile.lon_bucket
                )),
            });
        }
        // outcome_tx drops here → main thread's rx loop exits
    });

    // ── Drain results, report progress ────────────────────────────────────────
    let mut built = 0usize;
    let mut errors = 0usize;
    for outcome in rx {
        let done = done_count.load(Ordering::Relaxed);
        let frac = done as f32 / to_build.max(1) as f32;
        match outcome {
            Outcome::Built(min_lat, max_lat, min_lon, max_lon) => {
                built += 1;
                progress(ContourBuildProgress::built(
                    "Building",
                    frac,
                    format!("{done}/{to_build} tiles built"),
                    (min_lat, max_lat, min_lon, max_lon),
                ));
            }
            Outcome::Error(msg) => {
                errors += 1;
                progress(ContourBuildProgress::error("Building", frac, msg));
            }
        }
    }

    // Checkpoint the WAL so all committed tiles are in the main .sqlite file
    // (not just the WAL file).  This ensures the file size reflects actual data
    // and the DB is portable without the .sqlite-wal side-car.
    if let Ok(conn) = Connection::open(cache_db_path) {
        let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)");
    }

    Ok(format!(
        "Contours complete: {built} built, {skipped} cached, {errors} errors, {total} total"
    ))
}

// ── Tile range helpers ────────────────────────────────────────────────────────

fn bucket_range(coord_min: f32, coord_max: f32, step: f32) -> std::ops::RangeInclusive<i32> {
    let lo = (coord_min / step).floor() as i32;
    let hi = (coord_max / step).ceil() as i32;
    lo..=hi
}

