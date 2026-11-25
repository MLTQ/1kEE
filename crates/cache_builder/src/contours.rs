/// Offline contour pre-builder — Phase A (GDAL pipeline, SQLite output).
///
/// Iterates every tile in `zoom_buckets` that covers the requested bounding
/// box, skips tiles already present in `srtm_native_v1.sqlite`, and runs
/// the same gdalwarp + gdal_contour pipeline the desktop uses on-demand.
/// The desktop will find the pre-built tiles and skip its own generation.
use rayon::prelude::*;
use rusqlite::{Connection, OptionalExtension, params};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use tile_archive::contour_grid::{Bounds as CoreBounds, CoreTile};

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
        FocusContourSpec {
            half_extent_deg: 3.6,
            raster_size: 25920,
            interval_m: 200,
            zoom_bucket: 0,
        },
        FocusContourSpec {
            half_extent_deg: 2.2,
            raster_size: 15840,
            interval_m: 100,
            zoom_bucket: 1,
        },
        FocusContourSpec {
            half_extent_deg: 1.4,
            raster_size: 10080,
            interval_m: 50,
            zoom_bucket: 2,
        },
        FocusContourSpec {
            half_extent_deg: 0.9,
            raster_size: 6480,
            interval_m: 25,
            zoom_bucket: 3,
        },
        FocusContourSpec {
            half_extent_deg: 0.55,
            raster_size: 3960,
            interval_m: 10,
            zoom_bucket: 4,
        },
        FocusContourSpec {
            half_extent_deg: 0.3,
            raster_size: 2160,
            interval_m: 5,
            zoom_bucket: 5,
        },
        FocusContourSpec {
            half_extent_deg: 0.16,
            raster_size: 1152,
            interval_m: 5,
            zoom_bucket: 6,
        },
    ]
}

// ── SQLite helpers (mirrors desktop db.rs) ────────────────────────────────────

const CACHE_DB_NAME: &str = tile_archive::contour_grid::SRTM_NATIVE_DB_NAME;
const TEMP_DIR_NAME: &str = "srtm_focus_tmp";

pub fn default_cache_db_path(derived_terrain_dir: &Path) -> PathBuf {
    derived_terrain_dir.join(CACHE_DB_NAME)
}

pub fn open_cache_db(path: &Path) -> rusqlite::Result<Connection> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).ok();
    }
    let conn = Connection::open(path)?;
    conn.busy_timeout(Duration::from_secs(30))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    conn.pragma_update(None, "cache_size", -65536)?;
    let _ = conn.pragma_update(None, "mmap_size", 268_435_456i64);
    conn.execute_batch(
        "
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
    ",
    )?;
    Ok(conn)
}

pub fn tile_exists(conn: &Connection, tile: TileKey) -> bool {
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

/// Returns the `contour_count` for a tile, or `None` if the tile is not in
/// the manifest at all.  A count of 0 means the tile was explicitly marked
/// empty (no source coverage); count > 0 means it has real contour data.
pub fn tile_contour_count(conn: &Connection, tile: TileKey) -> rusqlite::Result<Option<i64>> {
    conn.query_row(
        "SELECT contour_count FROM contour_tile_manifest
         WHERE zoom_bucket=?1 AND lat_bucket=?2 AND lon_bucket=?3 LIMIT 1",
        params![tile.zoom_bucket, tile.lat_bucket, tile.lon_bucket],
        |row| row.get::<_, i64>(0),
    )
    .optional()
}

pub(crate) fn load_manifest_set(
    conn: &Connection,
) -> rusqlite::Result<std::collections::HashSet<(i32, i32, i32)>> {
    let mut stmt = conn.prepare(
        "SELECT zoom_bucket, lat_bucket, lon_bucket FROM contour_tile_manifest",
    )?;
    let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
    let mut out = std::collections::HashSet::new();
    for r in rows {
        out.insert(r?);
    }
    Ok(out)
}

pub(crate) fn load_manifest_counts(
    conn: &Connection,
) -> rusqlite::Result<std::collections::HashMap<(i32, i32, i32), i64>> {
    let mut stmt = conn.prepare(
        "SELECT zoom_bucket, lat_bucket, lon_bucket, contour_count FROM contour_tile_manifest",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(((row.get(0)?, row.get(1)?, row.get(2)?), row.get(3)?))
    })?;
    let mut out = std::collections::HashMap::new();
    for r in rows {
        let (k, v) = r?;
        out.insert(k, v);
    }
    Ok(out)
}

fn parse_srtm_tile_key(name: &str) -> Option<(i32, i32)> {
    let stem = name.strip_suffix(".tif")?;
    let b = stem.as_bytes();
    if b.len() != 7 {
        return None;
    }
    let lat_sign = match b[0] {
        b'N' => 1,
        b'S' => -1,
        _ => return None,
    };
    let lon_sign = match b[3] {
        b'E' => 1,
        b'W' => -1,
        _ => return None,
    };
    let lat = std::str::from_utf8(&b[1..3]).ok()?.parse::<i32>().ok()?;
    let lon = std::str::from_utf8(&b[4..7]).ok()?.parse::<i32>().ok()?;
    Some((lat * lat_sign, lon * lon_sign))
}

fn scan_srtm_tiles(srtm_root: &Path) -> (usize, std::collections::HashSet<(i32, i32)>) {
    let mut total_tifs = 0usize;
    let mut keys = std::collections::HashSet::new();
    if let Ok(entries) = fs::read_dir(srtm_root) {
        for entry in entries.flatten() {
            let fname_os = entry.file_name();
            let fname = fname_os.to_string_lossy();
            if fname.ends_with(".tif") {
                total_tifs += 1;
                if let Some(k) = parse_srtm_tile_key(&fname) {
                    keys.insert(k);
                }
            }
        }
    }
    (total_tifs, keys)
}

fn srtm_tile_paths_from_set(
    srtm_root: &Path,
    available: &std::collections::HashSet<(i32, i32)>,
    bounds: GeoBounds,
) -> Vec<PathBuf> {
    let mut tiles = Vec::new();
    for lat in (bounds.min_lat.floor() as i32)..=(bounds.max_lat.floor() as i32) {
        for lon in (bounds.min_lon.floor() as i32)..=(bounds.max_lon.floor() as i32) {
            if !available.contains(&(lat, lon)) {
                continue;
            }
            let ns = if lat >= 0 { 'N' } else { 'S' };
            let ew = if lon >= 0 { 'E' } else { 'W' };
            tiles.push(srtm_root.join(format!(
                "{}{:02}{}{:03}.tif",
                ns,
                lat.unsigned_abs(),
                ew,
                lon.unsigned_abs()
            )));
        }
    }
    tiles
}

fn has_srtm_in_bounds(
    available: &std::collections::HashSet<(i32, i32)>,
    bounds: GeoBounds,
) -> bool {
    for lat in (bounds.min_lat.floor() as i32)..=(bounds.max_lat.floor() as i32) {
        for lon in (bounds.min_lon.floor() as i32)..=(bounds.max_lon.floor() as i32) {
            if available.contains(&(lat, lon)) {
                return true;
            }
        }
    }
    false
}

pub(crate) fn read_gpkg_contours_clipped(
    gpkg_path: &Path,
    bounds: Option<CoreBounds>,
) -> rusqlite::Result<Vec<(i64, f32, Vec<u8>)>> {
    let source = Connection::open_with_flags(gpkg_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    source.busy_timeout(Duration::from_secs(30))?;
    let table_exists: bool = source
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='contour'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map(|n| n > 0)
        .unwrap_or(false);
    if !table_exists {
        return Ok(Vec::new());
    }
    let mut stmt = source
        .prepare("SELECT fid, geom, elevation_m FROM contour ORDER BY ABS(elevation_m), fid")?;
    let mut rows = stmt.query([])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let fid: i64 = row.get(0)?;
        let geom = row.get_ref(1)?.as_blob()?;
        let blob = match bounds {
            Some(b) => {
                let Some(clipped) = tile_archive::contour_clip::clip_gpkg(geom, b)
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(e.into()))?
                else {
                    continue;
                };
                clipped
            }
            None => geom.to_vec(),
        };
        let elev: f32 = row.get(2)?;
        out.push((fid, elev, blob));
    }
    Ok(out)
}

fn read_gpkg_coastlines_clipped(
    gpkg_path: &Path,
    bounds: Option<CoreBounds>,
) -> rusqlite::Result<Vec<(i64, Vec<u8>)>> {
    let source = Connection::open_with_flags(gpkg_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    source.busy_timeout(Duration::from_secs(30))?;
    let table_exists: bool = source
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='contour'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map(|n| n > 0)
        .unwrap_or(false);
    if !table_exists {
        return Ok(Vec::new());
    }
    let mut stmt = source.prepare("SELECT fid, geom FROM contour ORDER BY fid")?;
    let mut rows = stmt.query([])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let fid: i64 = row.get(0)?;
        let geom = row.get_ref(1)?.as_blob()?;
        let blob = match bounds {
            Some(b) => {
                let Some(clipped) = tile_archive::contour_clip::clip_gpkg(geom, b)
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(e.into()))?
                else {
                    continue;
                };
                clipped
            }
            None => geom.to_vec(),
        };
        out.push((fid, blob));
    }
    Ok(out)
}

pub(crate) fn write_encoded_tile(
    conn: &mut Connection,
    tile: TileKey,
    encoded_contours: &[(i64, f32, Vec<u8>)],
    encoded_coastlines: Option<&[(i64, Vec<u8>)]>,
) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    tx.execute(
        "DELETE FROM contour_tiles WHERE zoom_bucket=?1 AND lat_bucket=?2 AND lon_bucket=?3",
        params![tile.zoom_bucket, tile.lat_bucket, tile.lon_bucket],
    )?;
    tx.execute(
        "DELETE FROM contour_tile_manifest WHERE zoom_bucket=?1 AND lat_bucket=?2 AND lon_bucket=?3",
        params![tile.zoom_bucket, tile.lat_bucket, tile.lon_bucket],
    )?;
    {
        let mut insert_contour = tx.prepare_cached(
            "INSERT INTO contour_tiles (zoom_bucket,lat_bucket,lon_bucket,fid,elevation_m,geom)
             VALUES (?1,?2,?3,?4,?5,?6)",
        )?;
        for (fid, elevation_m, blob) in encoded_contours {
            insert_contour.execute(params![
                tile.zoom_bucket,
                tile.lat_bucket,
                tile.lon_bucket,
                fid,
                elevation_m,
                blob.as_slice()
            ])?;
        }
    }
    tx.execute(
        "INSERT INTO contour_tile_manifest (zoom_bucket,lat_bucket,lon_bucket,contour_count,built_at)
         VALUES (?1,?2,?3,?4,unixepoch())",
        params![
            tile.zoom_bucket,
            tile.lat_bucket,
            tile.lon_bucket,
            encoded_contours.len() as i64
        ],
    )?;

    if let Some(coastlines) = encoded_coastlines {
        tx.execute(
            "DELETE FROM coastline_tiles WHERE zoom_bucket=?1 AND lat_bucket=?2 AND lon_bucket=?3",
            params![tile.zoom_bucket, tile.lat_bucket, tile.lon_bucket],
        )?;
        tx.execute(
            "DELETE FROM coastline_tile_manifest WHERE zoom_bucket=?1 AND lat_bucket=?2 AND lon_bucket=?3",
            params![tile.zoom_bucket, tile.lat_bucket, tile.lon_bucket],
        )?;
        {
            let mut insert_coast = tx.prepare_cached(
                "INSERT INTO coastline_tiles (zoom_bucket,lat_bucket,lon_bucket,fid,geom)
                 VALUES (?1,?2,?3,?4,?5)",
            )?;
            for (fid, blob) in coastlines {
                insert_coast.execute(params![
                    tile.zoom_bucket,
                    tile.lat_bucket,
                    tile.lon_bucket,
                    fid,
                    blob.as_slice()
                ])?;
            }
        }
        tx.execute(
            "INSERT INTO coastline_tile_manifest (zoom_bucket,lat_bucket,lon_bucket,line_count)
             VALUES (?1,?2,?3,?4)",
            params![
                tile.zoom_bucket,
                tile.lat_bucket,
                tile.lon_bucket,
                coastlines.len() as i64
            ],
        )?;
    }

    tx.commit()
}

pub fn import_tile(cache_db_path: &Path, tile: TileKey, gpkg_path: &Path) -> rusqlite::Result<()> {
    import_tile_clipped(cache_db_path, tile, gpkg_path, None)
}

fn import_tile_clipped(
    cache_db_path: &Path,
    tile: TileKey,
    gpkg_path: &Path,
    bounds: Option<CoreBounds>,
) -> rusqlite::Result<()> {
    let encoded = read_gpkg_contours_clipped(gpkg_path, bounds)?;
    let mut cache = open_cache_db(cache_db_path)?;
    write_encoded_tile(&mut cache, tile, &encoded, None)
}

// ── GDAL helpers (mirrors desktop gdal.rs) ────────────────────────────────────

pub(crate) fn run_gdal(mut cmd: Command, label: &str) -> std::io::Result<()> {
    let start = Instant::now();
    let timeout = Duration::from_secs(120);
    let mut child = cmd.spawn()?;
    let mut delay_ms = 2u64;
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
                format!("{label} timed out"),
            ));
        }
        std::thread::sleep(Duration::from_millis(delay_ms));
        delay_ms = (delay_ms * 2).min(50);
    }
}

fn run_gdalwarp(
    gdalwarp: &Path,
    tiles: &[PathBuf],
    out_tif: &Path,
    bounds: CoreBounds,
    spec: FocusContourSpec,
) -> std::io::Result<()> {
    let mut cmd = Command::new(gdalwarp);
    cmd.args([
        "-q",
        "-overwrite",
        "-r",
        "bilinear",
        "-dstnodata",
        "-32768",
        "-wo",
        "NUM_THREADS=1",
        "--config",
        "GDAL_PAM_ENABLED",
        "NO",
        "-te",
        &format!("{:.15}", bounds.min_lon),
        &format!("{:.15}", bounds.min_lat),
        &format!("{:.15}", bounds.max_lon),
        &format!("{:.15}", bounds.max_lat),
        "-ts",
        &spec.raster_size.to_string(),
        &spec.raster_size.to_string(),
    ]);
    for tile in tiles {
        cmd.arg(tile);
    }
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
    cmd.args([
        "-q",
        "-f",
        "GPKG",
        "-a",
        "elevation_m",
        "-i",
        &interval_m.to_string(),
        "-snodata",
        "-32768",
        "-nln",
        "contour",
        "--config",
        "GDAL_PAM_ENABLED",
        "NO",
        "--config",
        "OGR_SQLITE_SYNCHRONOUS",
        "OFF",
        "--config",
        "OGR_SQLITE_CACHE",
        "64",
    ]);
    cmd.arg(in_tif);
    cmd.arg(out_gpkg);
    run_gdal(cmd, "gdal_contour")
}

fn run_gdal_coastline(gdal_contour: &Path, in_tif: &Path, out_gpkg: &Path) -> std::io::Result<()> {
    let mut cmd = Command::new(gdal_contour);
    cmd.args([
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
        "--config",
        "GDAL_PAM_ENABLED",
        "NO",
        "--config",
        "OGR_SQLITE_SYNCHRONOUS",
        "OFF",
        "--config",
        "OGR_SQLITE_CACHE",
        "64",
    ]);
    cmd.arg(in_tif);
    cmd.arg(out_gpkg);
    run_gdal(cmd, "gdal_contour (coastline 0m)")
}

fn cleanup(paths: &[&Path]) {
    for p in paths {
        let _ = fs::remove_file(p);
    }
}

// ── Public entry point ────────────────────────────────────────────────────────

pub struct ContourBuildProgress {
    pub stage: String,
    pub fraction: f32,
    pub message: String,
    pub is_error: bool,
    /// Set when a tile was just completed; carries (min_lat, max_lat, min_lon, max_lon).
    pub tile_bounds: Option<(f32, f32, f32, f32)>,
}

impl ContourBuildProgress {
    pub fn info(stage: impl Into<String>, fraction: f32, message: impl Into<String>) -> Self {
        Self {
            stage: stage.into(),
            fraction,
            message: message.into(),
            is_error: false,
            tile_bounds: None,
        }
    }
    pub fn error(stage: impl Into<String>, fraction: f32, message: impl Into<String>) -> Self {
        Self {
            stage: stage.into(),
            fraction,
            message: message.into(),
            is_error: true,
            tile_bounds: None,
        }
    }
    pub fn built(
        stage: impl Into<String>,
        fraction: f32,
        message: impl Into<String>,
        bounds: (f32, f32, f32, f32),
    ) -> Self {
        Self {
            stage: stage.into(),
            fraction,
            message: message.into(),
            is_error: false,
            tile_bounds: Some(bounds),
        }
    }
}

/// Resolve the full path to a GDAL tool, checking the explicit bin dir first
/// then common Homebrew locations, then falling back to bare name (PATH).
pub fn resolve_gdal_tool(gdal_bin_dir: &Path, tool: &str) -> PathBuf {
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
/// reads as `Derived/terrain/srtm_native_v1.sqlite`).
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
    let (srtm_tile_count, srtm_set) = scan_srtm_tiles(srtm_root);
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
        match std::process::Command::new(tool_path)
            .arg("--version")
            .output()
        {
            Ok(out) if out.status.success() => {
                let version = String::from_utf8_lossy(&out.stdout);
                progress(ContourBuildProgress::info(
                    "Startup",
                    0.0,
                    format!("{name}: {}", version.trim()),
                ));
            }
            Ok(_) => {
                return Err(format!(
                    "{name} at '{}' returned an error on --version",
                    tool_path.display()
                ));
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
    let native_db = open_cache_db(cache_db_path).map_err(|e| e.to_string())?;
    tile_archive::native_contours::initialize(&native_db).map_err(|e| e.to_string())?;
    drop(native_db);

    let specs = all_specs();
    let selected: Vec<FocusContourSpec> = specs
        .iter()
        .filter(|s| zoom_buckets.contains(&s.zoom_bucket))
        .copied()
        .collect();

    // ── Phase 1: collect work (single-threaded, needs SQLite connection) ──────
    progress(ContourBuildProgress::info(
        "Planning",
        0.0,
        "Scanning tiles…",
    ));

    struct TileWork {
        tile: TileKey,
        spec: FocusContourSpec,
        srtm_tiles: Vec<PathBuf>,
    }

    let conn = open_cache_db(cache_db_path).map_err(|e| e.to_string())?;
    let existing = load_manifest_set(&conn).unwrap_or_default();
    drop(conn);
    let mut work: Vec<TileWork> = Vec::new();
    let mut skipped = 0usize;

    for spec in &selected {
        let bucket_step = spec.half_extent_deg * 0.45;
        for lat_bucket in bucket_range(bounds.min_lat, bounds.max_lat, bucket_step) {
            for lon_bucket in bucket_range(bounds.min_lon, bounds.max_lon, bucket_step) {
                let tile = TileKey {
                    zoom_bucket: spec.zoom_bucket,
                    lat_bucket,
                    lon_bucket,
                };
                if existing.contains(&(tile.zoom_bucket, tile.lat_bucket, tile.lon_bucket)) {
                    skipped += 1;
                    continue;
                }
                let source = CoreTile::srtm(spec.half_extent_deg, lat_bucket, lon_bucket).source;
                let tile_bounds = GeoBounds {
                    min_lat: source.min_lat as f32,
                    max_lat: source.max_lat as f32,
                    min_lon: source.min_lon as f32,
                    max_lon: source.max_lon as f32,
                };
                let srtm_tiles = srtm_tile_paths_from_set(srtm_root, &srtm_set, tile_bounds);
                if !srtm_tiles.is_empty() {
                    work.push(TileWork {
                        tile,
                        spec: *spec,
                        srtm_tiles,
                    });
                }
            }
        }
    }

    let total = work.len() + skipped;
    let to_build = work.len();
    progress(ContourBuildProgress::info(
        "Planning",
        0.0,
        format!("{to_build} tiles to build, {skipped} already cached, {total} total"),
    ));

    if work.is_empty() {
        return Ok(format!(
            "Contours complete: 0 built, {skipped} already cached, {total} total"
        ));
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

    enum Outcome {
        Built(f32, f32, f32, f32),
        Error(String),
    }
    type GdalReady = Result<
        (
            TileKey,
            CoreBounds,
            Vec<(i64, f32, Vec<u8>)>,
            Option<Vec<(i64, Vec<u8>)>>,
        ),
        String,
    >;
    let (write_tx, write_rx) = mpsc::sync_channel::<GdalReady>(32);
    let (tx, rx) = mpsc::channel::<Outcome>();
    let done_count = Arc::new(AtomicUsize::new(0));

    // Capture by-value for the spawned threads
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
            work.into_par_iter().for_each_with(write_tx, |tx, w| {
                let TileWork {
                    tile,
                    spec,
                    srtm_tiles,
                } = w;
                let stem = format!(
                    "z{}_lat{}_lon{}",
                    tile.zoom_bucket, tile.lat_bucket, tile.lon_bucket
                );
                let tmp_tif = tmp_dir.join(format!("{stem}.tmp.tif"));
                let tmp_gpkg = tmp_dir.join(format!("{stem}.tmp.gpkg"));
                let tmp_coast_gpkg = tmp_dir.join(format!("{stem}.coast.tmp.gpkg"));
                cleanup(&[&tmp_tif, &tmp_gpkg, &tmp_coast_gpkg]);

                let core = CoreTile::srtm(spec.half_extent_deg, tile.lat_bucket, tile.lon_bucket);
                let ready = (|| {
                    run_gdalwarp(
                        &gdalwarp,
                        &srtm_tiles,
                        &tmp_tif,
                        core.source,
                        FocusContourSpec {
                            raster_size: core.raster_size,
                            ..spec
                        },
                    )
                    .map_err(|e| format!("gdalwarp: {e}"))?;
                    run_gdal_contour(&gdal_contour, &tmp_tif, &tmp_gpkg, spec.interval_m)
                        .map_err(|e| format!("gdal_contour: {e}"))?;
                    let contours = read_gpkg_contours_clipped(&tmp_gpkg, Some(core.core))
                        .map_err(|e| format!("gpkg read: {e}"))?;
                    let coastlines = if run_gdal_coastline(&gdal_contour, &tmp_tif, &tmp_coast_gpkg)
                        .is_ok()
                    {
                        read_gpkg_coastlines_clipped(&tmp_coast_gpkg, Some(core.core)).ok()
                    } else {
                        None
                    };
                    Ok((tile, core.core, contours, coastlines))
                })()
                .map_err(|e: String| {
                    format!(
                        "z{} ({},{}) — {e}",
                        tile.zoom_bucket, tile.lat_bucket, tile.lon_bucket
                    )
                });

                cleanup(&[&tmp_tif, &tmp_gpkg, &tmp_coast_gpkg]);
                let _ = tx.send(ready);
            });
        });
    });

    // Dedicated SQLite writer thread: single persistent Connection, zero writer-lock contention.
    std::thread::spawn(move || {
        let mut conn = match open_cache_db(&cache_db_path) {
            Ok(c) => c,
            Err(e) => {
                let _ = tx.send(Outcome::Error(format!("open cache DB failed: {e}")));
                return;
            }
        };
        for item in write_rx {
            done_arc.fetch_add(1, Ordering::Relaxed);
            let outcome = match item {
                Ok((tile, core_bounds, contours, coastlines)) => {
                    match write_encoded_tile(&mut conn, tile, &contours, coastlines.as_deref()) {
                        Ok(()) => Outcome::Built(
                            core_bounds.min_lat as f32,
                            core_bounds.max_lat as f32,
                            core_bounds.min_lon as f32,
                            core_bounds.max_lon as f32,
                        ),
                        Err(e) => Outcome::Error(format!(
                            "z{} ({},{}) — db import: {e}",
                            tile.zoom_bucket, tile.lat_bucket, tile.lon_bucket
                        )),
                    }
                }
                Err(msg) => Outcome::Error(msg),
            };
            let _ = tx.send(outcome);
        }
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
pub(crate) fn write_tile_native(
    conn: &mut Connection,
    tile: TileKey,
    contours: &[crate::marching_squares::ContourLine],
    coastlines: &[Vec<(f32, f32)>],
) -> rusqlite::Result<()> {
    let core_bounds = earth_core_bounds(tile);

    // Clip and encode WKB directly in parallel BEFORE taking the SQLite write transaction,
    // eliminating the encode -> decode -> re-encode roundtrip and minimizing lock duration.
    let encoded_contours: Vec<(i64, f32, Vec<u8>)> = contours
        .par_iter()
        .enumerate()
        .filter_map(|(fid, line)| {
            let blob = match core_bounds {
                Some(bounds) => tile_archive::contour_clip::clip_points_f32(&line.points, bounds)?,
                None => encode_gpkg_linestring(&line.points),
            };
            Some((fid as i64, line.elevation_m, blob))
        })
        .collect();

    let encoded_coastlines: Vec<(i64, Vec<u8>)> = coastlines
        .par_iter()
        .enumerate()
        .filter_map(|(fid, pts)| {
            let blob = match core_bounds {
                Some(bounds) => tile_archive::contour_clip::clip_points_f32(pts, bounds)?,
                None => encode_gpkg_linestring(pts),
            };
            Some((fid as i64, blob))
        })
        .collect();

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
    {
        let mut insert_contour = tx.prepare_cached(
            "INSERT INTO contour_tiles (zoom_bucket,lat_bucket,lon_bucket,fid,elevation_m,geom)
             VALUES (?1,?2,?3,?4,?5,?6)",
        )?;
        for (fid, elevation_m, blob) in &encoded_contours {
            insert_contour.execute(params![
                tile.zoom_bucket,
                tile.lat_bucket,
                tile.lon_bucket,
                fid,
                elevation_m,
                blob
            ])?;
        }
    }
    tx.execute(
        "INSERT INTO contour_tile_manifest (zoom_bucket,lat_bucket,lon_bucket,contour_count,built_at)
         VALUES (?1,?2,?3,?4,unixepoch())",
        params![
            tile.zoom_bucket,
            tile.lat_bucket,
            tile.lon_bucket,
            encoded_contours.len() as i64
        ],
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
    {
        let mut insert_coast = tx.prepare_cached(
            "INSERT INTO coastline_tiles (zoom_bucket,lat_bucket,lon_bucket,fid,geom)
             VALUES (?1,?2,?3,?4,?5)",
        )?;
        for (fid, blob) in &encoded_coastlines {
            insert_coast.execute(params![
                tile.zoom_bucket,
                tile.lat_bucket,
                tile.lon_bucket,
                fid,
                blob
            ])?;
        }
    }
    tx.execute(
        "INSERT INTO coastline_tile_manifest (zoom_bucket,lat_bucket,lon_bucket,line_count)
         VALUES (?1,?2,?3,?4)",
        params![
            tile.zoom_bucket,
            tile.lat_bucket,
            tile.lon_bucket,
            encoded_coastlines.len() as i64
        ],
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
    use crate::marching_squares::{
        NativeSrtmSampler, SharedSrtmCache, build_tile_contours_on_grid,
    };

    // ── Validate SRTM root ────────────────────────────────────────────────────
    if !srtm_root.exists() {
        return Err(format!("SRTM root does not exist: {}", srtm_root.display()));
    }
    let (srtm_tile_count, srtm_set) = scan_srtm_tiles(srtm_root);
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

    let native_db = open_cache_db(cache_db_path).map_err(|e| e.to_string())?;
    tile_archive::native_contours::initialize(&native_db).map_err(|e| e.to_string())?;
    let existing = load_manifest_set(&native_db).unwrap_or_default();
    drop(native_db);

    let specs = all_specs();
    let selected: Vec<FocusContourSpec> = specs
        .iter()
        .filter(|s| zoom_buckets.contains(&s.zoom_bucket))
        .copied()
        .collect();

    // ── Planning phase ────────────────────────────────────────────────────────
    progress(ContourBuildProgress::info(
        "Planning",
        0.0,
        "Scanning tiles…",
    ));

    let mut work: Vec<(TileKey, GeoBounds, FocusContourSpec)> = Vec::new();
    let mut skipped = 0usize;

    for spec in &selected {
        let bucket_step = spec.half_extent_deg * 0.45;
        for lat_bucket in bucket_range(bounds.min_lat, bounds.max_lat, bucket_step) {
            for lon_bucket in bucket_range(bounds.min_lon, bounds.max_lon, bucket_step) {
                let tile = TileKey {
                    zoom_bucket: spec.zoom_bucket,
                    lat_bucket,
                    lon_bucket,
                };
                if existing.contains(&(tile.zoom_bucket, tile.lat_bucket, tile.lon_bucket)) {
                    skipped += 1;
                    continue;
                }
                let source = CoreTile::srtm(spec.half_extent_deg, lat_bucket, lon_bucket).source;
                let tile_bounds = GeoBounds {
                    min_lat: source.min_lat as f32,
                    max_lat: source.max_lat as f32,
                    min_lon: source.min_lon as f32,
                    max_lon: source.max_lon as f32,
                };
                // Only plan tiles that have at least one SRTM tile in range.
                if has_srtm_in_bounds(&srtm_set, tile_bounds) {
                    work.push((tile, tile_bounds, *spec));
                }
            }
        }
    }

    // Interleave zoom levels so all levels progress simultaneously — avoids the
    // visual artifact where low-zoom (large) tiles complete first and flood the
    // map with solid blocks before high-zoom tiles even start.
    // Round-robin: take tile[0] from z0, tile[0] from z1, …, tile[1] from z0, …
    {
        let mut by_zoom: std::collections::BTreeMap<i32, Vec<_>> =
            std::collections::BTreeMap::new();
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
        return Ok(format!(
            "All {total} tiles already cached — nothing to build"
        ));
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

    // compute workers → writer thread (pre-clipped & pre-encoded WKB so writer only inserts)
    type ComputeResult = (
        TileKey,
        GeoBounds,
        Vec<(i64, f32, Vec<u8>)>,
        Vec<(i64, Vec<u8>)>,
    );
    let (compute_tx, compute_rx) = mpsc::sync_channel::<ComputeResult>(32);

    // writer thread → main thread (outcome events)
    enum Outcome {
        Built(f32, f32, f32, f32),
        Error(String),
    }
    let (outcome_tx, rx) = mpsc::channel::<Outcome>();
    let done_count = Arc::new(AtomicUsize::new(0));

    let shared_srtm = SharedSrtmCache::new(srtm_root.to_path_buf(), 64);

    // ── Compute thread: fans work out to rayon with shared SRTM LRU ──────────
    std::thread::spawn(move || {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(num_threads)
            .build()
            .expect("rayon pool");
        pool.install(|| {
            work.into_par_iter()
                .map_init(
                    || NativeSrtmSampler::with_shared(shared_srtm.clone()),
                    |sampler, (tile, _tile_bounds, spec)| {
                        let core =
                            CoreTile::srtm(spec.half_extent_deg, tile.lat_bucket, tile.lon_bucket);
                        let (contours, coastlines) = build_tile_contours_on_grid(
                            FocusContourSpec {
                                raster_size: core.raster_size,
                                ..spec
                            },
                            core.source,
                            |lat, lon| sampler.sample(lat, lon),
                        );
                        let encoded_contours: Vec<(i64, f32, Vec<u8>)> = contours
                            .iter()
                            .enumerate()
                            .filter_map(|(fid, line)| {
                                let blob = tile_archive::contour_clip::clip_points_f32(
                                    &line.points,
                                    core.core,
                                )?;
                                Some((fid as i64, line.elevation_m, blob))
                            })
                            .collect();
                        let encoded_coastlines: Vec<(i64, Vec<u8>)> = coastlines
                            .iter()
                            .enumerate()
                            .filter_map(|(fid, pts)| {
                                let blob =
                                    tile_archive::contour_clip::clip_points_f32(pts, core.core)?;
                                Some((fid as i64, blob))
                            })
                            .collect();
                        let display = GeoBounds {
                            min_lat: core.core.min_lat as f32,
                            max_lat: core.core.max_lat as f32,
                            min_lon: core.core.min_lon as f32,
                            max_lon: core.core.max_lon as f32,
                        };
                        (tile, display, encoded_contours, encoded_coastlines)
                    },
                )
                .for_each_with(compute_tx, |tx, result| {
                    let _ = tx.send(result);
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
        for (tile, tile_bounds, encoded_contours, encoded_coastlines) in compute_rx {
            let outcome = write_encoded_tile(
                &mut conn,
                tile,
                &encoded_contours,
                Some(&encoded_coastlines),
            );
            done_arc.fetch_add(1, Ordering::Relaxed);
            let _ = outcome_tx.send(match outcome {
                Ok(()) => Outcome::Built(
                    tile_bounds.min_lat,
                    tile_bounds.max_lat,
                    tile_bounds.min_lon,
                    tile_bounds.max_lon,
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

pub fn bucket_range(coord_min: f32, coord_max: f32, step: f32) -> std::ops::RangeInclusive<i32> {
    let lo = (coord_min / step).floor() as i32;
    let hi = (coord_max / step).ceil() as i32;
    lo..=hi
}

fn earth_core_bounds(tile: TileKey) -> Option<CoreBounds> {
    all_specs()
        .into_iter()
        .find(|s| s.zoom_bucket == tile.zoom_bucket)
        .map(|s| {
            CoreTile::srtm(s.half_extent_deg, tile.lat_bucket, tile.lon_bucket)
            .core
        })
}
fn clip_geometry(blob: Vec<u8>, bounds: Option<CoreBounds>) -> rusqlite::Result<Option<Vec<u8>>> {
    match bounds {
        Some(bounds) => tile_archive::contour_clip::clip_gpkg(&blob, bounds)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(e.into())),
        None => Ok(Some(blob)),
    }
}

#[cfg(test)]
#[path = "contour_core_tests.rs"]
mod core_tests;
