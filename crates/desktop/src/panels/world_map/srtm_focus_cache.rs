use crate::model::GeoPoint;
use crate::terrain_assets;
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::HashSet; // used by pending_set()
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const BUILD_TIMEOUT: Duration = Duration::from_secs(90);
const CACHE_DB_NAME: &str = "srtm_focus_cache.sqlite";
const TEMP_DIR_NAME: &str = "srtm_focus_tmp";
const MAX_BACKGROUND_BUILDS: usize = 2;

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

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
struct TileKey {
    zoom_bucket: i32,
    lat_bucket: i32,
    lon_bucket: i32,
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

pub fn zoom_bucket_for_zoom(zoom: f32) -> i32 {
    spec_for_zoom(zoom).zoom_bucket
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
    let Some(cache_db_path) = focus_cache_db_path(selected_root) else {
        return Vec::new();
    };
    if ensure_cache_schema(&cache_db_path).is_err() {
        return Vec::new();
    }
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
                &cache_db_path,
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
    let cache_db_path = focus_cache_db_path(selected_root)?;
    let connection = open_cache_db(&cache_db_path).ok()?;
    let spec = spec_for_zoom(zoom);
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
            if tile_exists(&connection, tile).ok()? {
                ready_assets += 1;
            } else if is_pending(tile) {
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
    let cache_root = focus_cache_root(selected_root)?;
    let output_path = cache_root.join("global_land_overview.gpkg");

    if output_path.exists() {
        return Some(output_path);
    }

    if global_overview_building().load(Ordering::Relaxed) {
        return None;
    }

    // Prefer a pre-existing VRT in the parent of the SRTM tile directory
    // (many distributions ship one, e.g. SRTM_GL1_srtm.vrt alongside
    // SRTM_GL1_srtm/).  Failing that, scan for tiles and build our own.
    let source_vrt = find_prebuilt_vrt(&srtm_root);
    let tiles_for_vrt = if source_vrt.is_none() {
        let t = find_all_srtm_tiles(&srtm_root);
        if t.is_empty() {
            return None;
        }
        t
    } else {
        Vec::new()
    };

    global_overview_building().store(true, Ordering::SeqCst);
    let cache_root_clone = cache_root.clone();

    std::thread::spawn(move || {
        let tmp_dir = cache_root_clone.join(TEMP_DIR_NAME);
        let tmp_tif = tmp_dir.join("global_overview.tmp.tif");
        let tmp_gpkg = tmp_dir.join("global_overview.tmp.gpkg");
        let out = cache_root_clone.join("global_land_overview.gpkg");
        let _ = build_global_overview(source_vrt.as_deref(), &tiles_for_vrt, &tmp_tif, &tmp_gpkg, &out);
        global_overview_building().store(false, Ordering::SeqCst);
    });

    None
}

/// Look for a `.vrt` file adjacent to the SRTM tile directory that covers
/// the full dataset, e.g. `…/srtm_gl1/SRTM_GL1_srtm.vrt`.
fn find_prebuilt_vrt(srtm_root: &Path) -> Option<PathBuf> {
    let parent = srtm_root.parent()?;
    // Try the canonical name first, then any .vrt in the parent directory.
    let canonical = parent.join(format!(
        "{}.vrt",
        srtm_root.file_name()?.to_string_lossy()
    ));
    if canonical.exists() {
        return Some(canonical);
    }
    std::fs::read_dir(parent).ok()?.find_map(|e| {
        let p = e.ok()?.path();
        (p.extension()?.to_str() == Some("vrt")).then_some(p)
    })
}

fn find_all_srtm_tiles(root: &Path) -> Vec<PathBuf> {
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
fn build_global_overview(
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
        run_command_with_timeout(cmd, "gdalbuildvrt (global overview)", Duration::from_secs(120))
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
    run_command_with_timeout(cmd, "gdalwarp (global overview)", Duration::from_secs(600))
        .ok()?;
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
    run_command_with_timeout(cmd, "gdal_contour (global overview)", Duration::from_secs(300))
        .ok()?;

    // fs::rename fails across filesystems; fall back to copy+delete.
    if fs::rename(tmp_gpkg, output_path).is_err() {
        fs::copy(tmp_gpkg, output_path).ok()?;
        let _ = fs::remove_file(tmp_gpkg);
    }
    let _ = fs::remove_file(tmp_tif);
    Some(())
}

fn global_overview_building() -> &'static AtomicBool {
    static BUILDING: OnceLock<AtomicBool> = OnceLock::new();
    BUILDING.get_or_init(|| AtomicBool::new(false))
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
    cache_db_path: &Path,
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
    let tile = TileKey {
        zoom_bucket: spec.zoom_bucket,
        lat_bucket,
        lon_bucket,
    };

    if open_cache_db(cache_db_path)
        .and_then(|connection| tile_exists(&connection, tile))
        .ok()
        .unwrap_or(false)
    {
        return Some(FocusContourAsset {
            path: cache_db_path.to_path_buf(),
            simplify_step: spec.simplify_step,
            zoom_bucket: spec.zoom_bucket,
            lat_bucket,
            lon_bucket,
        });
    }

    if is_pending(tile) {
        return None;
    }

    if !try_acquire_build_slot() {
        return None;
    }

    let pending = pending_set();
    let mut guard = pending.lock().ok()?;
    if !guard.insert(tile) {
        release_build_slot();
        return None;
    }
    drop(guard);

    let srtm_root = srtm_root.to_path_buf();
    let cache_root = cache_root.to_path_buf();
    let cache_db_path = cache_db_path.to_path_buf();
    std::thread::spawn(move || {
        let _ = build_focus_contours(&srtm_root, &cache_root, &cache_db_path, tile, bounds, spec);
        if let Ok(mut guard) = pending_set().lock() {
            guard.remove(&tile);
        }
        release_build_slot();
    });

    None
}

fn focus_cache_root(selected_root: Option<&Path>) -> Option<PathBuf> {
    let root = terrain_assets::find_derived_root(selected_root)
        .unwrap_or_else(|| std::env::temp_dir().join("1kee-derived"));
    let cache_root = root.join("terrain");
    fs::create_dir_all(&cache_root).ok()?;
    Some(cache_root)
}

fn focus_cache_db_path(selected_root: Option<&Path>) -> Option<PathBuf> {
    Some(focus_cache_root(selected_root)?.join(CACHE_DB_NAME))
}

fn build_focus_contours(
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
    cleanup_temp_tile_artifacts(&tmp_tif_path, &tmp_gpkg_path);
    Some(())
}

fn open_cache_db(path: &Path) -> rusqlite::Result<Connection> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).ok();
    }
    let connection = Connection::open(path)?;
    connection.busy_timeout(Duration::from_secs(30))?;
    connection.pragma_update(None, "journal_mode", "WAL")?;
    connection.pragma_update(None, "synchronous", "NORMAL")?;
    connection.pragma_update(None, "temp_store", "MEMORY")?;
    ensure_cache_schema_with_connection(&connection)?;
    Ok(connection)
}

fn ensure_cache_schema(path: &Path) -> rusqlite::Result<()> {
    let _ = open_cache_db(path)?;
    Ok(())
}

fn ensure_cache_schema_with_connection(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS contour_tile_manifest (
            zoom_bucket INTEGER NOT NULL,
            lat_bucket INTEGER NOT NULL,
            lon_bucket INTEGER NOT NULL,
            contour_count INTEGER NOT NULL,
            built_at INTEGER NOT NULL DEFAULT (unixepoch()),
            PRIMARY KEY (zoom_bucket, lat_bucket, lon_bucket)
        );

        CREATE TABLE IF NOT EXISTS contour_tiles (
            zoom_bucket INTEGER NOT NULL,
            lat_bucket INTEGER NOT NULL,
            lon_bucket INTEGER NOT NULL,
            fid INTEGER NOT NULL,
            elevation_m REAL NOT NULL,
            geom BLOB NOT NULL,
            PRIMARY KEY (zoom_bucket, lat_bucket, lon_bucket, fid)
        );

        CREATE INDEX IF NOT EXISTS idx_contour_tiles_lookup
            ON contour_tiles (zoom_bucket, lat_bucket, lon_bucket, elevation_m, fid);
        ",
    )?;
    Ok(())
}

fn tile_exists(connection: &Connection, tile: TileKey) -> rusqlite::Result<bool> {
    connection
        .query_row(
            "SELECT 1
             FROM contour_tile_manifest
             WHERE zoom_bucket = ?1 AND lat_bucket = ?2 AND lon_bucket = ?3
             LIMIT 1",
            params![tile.zoom_bucket, tile.lat_bucket, tile.lon_bucket],
            |_row| Ok(()),
        )
        .optional()
        .map(|value| value.is_some())
}

fn import_tile_into_cache(
    cache_db_path: &Path,
    tile: TileKey,
    gpkg_path: &Path,
) -> rusqlite::Result<()> {
    let source = Connection::open(gpkg_path)?;
    source.busy_timeout(Duration::from_secs(30))?;
    let mut statement = source
        .prepare("SELECT fid, geom, elevation_m FROM contour ORDER BY ABS(elevation_m), fid")?;
    let mut rows = statement.query([])?;

    let mut cache = open_cache_db(cache_db_path)?;
    let transaction = cache.transaction()?;
    transaction.execute(
        "DELETE FROM contour_tiles
         WHERE zoom_bucket = ?1 AND lat_bucket = ?2 AND lon_bucket = ?3",
        params![tile.zoom_bucket, tile.lat_bucket, tile.lon_bucket],
    )?;
    transaction.execute(
        "DELETE FROM contour_tile_manifest
         WHERE zoom_bucket = ?1 AND lat_bucket = ?2 AND lon_bucket = ?3",
        params![tile.zoom_bucket, tile.lat_bucket, tile.lon_bucket],
    )?;

    let mut contour_count = 0usize;
    while let Some(row) = rows.next()? {
        let fid: i64 = row.get(0)?;
        let geometry: Vec<u8> = row.get(1)?;
        let elevation_m: f32 = row.get(2)?;
        transaction.execute(
            "INSERT INTO contour_tiles (
                 zoom_bucket,
                 lat_bucket,
                 lon_bucket,
                 fid,
                 elevation_m,
                 geom
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                tile.zoom_bucket,
                tile.lat_bucket,
                tile.lon_bucket,
                fid,
                elevation_m,
                geometry
            ],
        )?;
        contour_count += 1;
    }

    transaction.execute(
        "INSERT INTO contour_tile_manifest (
             zoom_bucket,
             lat_bucket,
             lon_bucket,
             contour_count,
             built_at
         ) VALUES (?1, ?2, ?3, ?4, unixepoch())",
        params![
            tile.zoom_bucket,
            tile.lat_bucket,
            tile.lon_bucket,
            contour_count as i64
        ],
    )?;
    transaction.commit()?;
    Ok(())
}

fn temp_tile_paths(cache_root: &Path, tile: TileKey) -> (PathBuf, PathBuf) {
    let temp_root = cache_root.join(TEMP_DIR_NAME);
    let stem = format!(
        "z{}_lat{}_lon{}",
        tile.zoom_bucket, tile.lat_bucket, tile.lon_bucket
    );
    (
        temp_root.join(format!("{stem}.tmp.tif")),
        temp_root.join(format!("{stem}.tmp.gpkg")),
    )
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

fn run_command(command: Command, label: &str) -> std::io::Result<()> {
    run_command_with_timeout(command, label, BUILD_TIMEOUT)
}

fn run_command_with_timeout(mut command: Command, label: &str, timeout: Duration) -> std::io::Result<()> {
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

fn cleanup_temp_tile_artifacts(tif_path: &Path, gpkg_path: &Path) {
    let _ = fs::remove_file(tif_path);
    let _ = fs::remove_file(gpkg_path);
    let _ = fs::remove_file(journal_path_for(gpkg_path));
    let _ = fs::remove_file(wal_path_for(gpkg_path));
    let _ = fs::remove_file(shm_path_for(gpkg_path));
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

fn pending_set() -> &'static Mutex<HashSet<TileKey>> {
    static PENDING: OnceLock<Mutex<HashSet<TileKey>>> = OnceLock::new();
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

fn active_build_slots() -> &'static AtomicUsize {
    static ACTIVE: OnceLock<AtomicUsize> = OnceLock::new();
    ACTIVE.get_or_init(|| AtomicUsize::new(0))
}

fn try_acquire_build_slot() -> bool {
    active_build_slots()
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |count| {
            (count < MAX_BACKGROUND_BUILDS).then_some(count + 1)
        })
        .is_ok()
}

fn release_build_slot() {
    let current = active_build_slots().load(Ordering::SeqCst);
    if current > 0 {
        active_build_slots().fetch_sub(1, Ordering::SeqCst);
    }
}

fn is_pending(tile: TileKey) -> bool {
    pending_set()
        .lock()
        .map(|guard| guard.contains(&tile))
        .unwrap_or(false)
}
