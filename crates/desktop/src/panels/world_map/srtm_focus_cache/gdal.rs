use super::db::{
    cleanup_temp_tile_artifacts, import_coastline_into_cache, import_tile_into_cache,
    journal_path_for, mark_tile_empty, shm_path_for, temp_tile_paths, wal_path_for,
};
use super::{BUILD_TIMEOUT, FocusContourSpec, GeoBounds, TEMP_DIR_NAME, TileKey};
use crate::settings_store;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const LUNAR_SOURCE_CHUNK_CENTER_STEP_DEG: f32 = 4.0;
const LUNAR_SOURCE_CHUNK_HALF_EXTENT_DEG: f32 = 6.0;
const LUNAR_SOURCE_CHUNK_DIR: &str = "lunar_source_chunks";

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

pub fn gebco_derived_building() -> &'static AtomicBool {
    static BUILDING: OnceLock<AtomicBool> = OnceLock::new();
    BUILDING.get_or_init(|| AtomicBool::new(false))
}

/// Build the GEBCO-derived runtime assets into `cache_root` (= derived/terrain/).
///
/// Three outputs are produced in order:
///   1. `gebco_2025_preview_4096.tif`  — 4096×2048 Int16 GeoTIFF (skipped if present)
///   2. `gebco_depth_1440x720.bil`     — 1440×720 Int16 EHdr grid for the depth-fill texture
///   3. `gebco_2025_contours_200m.gpkg`— 200 m isobath GeoPackage for bathymetry layer
///
/// Each output is skipped if it already exists, so a partial run (or a
/// manually-generated file) is always respected.
pub fn build_gebco_derived(tiles: &[PathBuf], cache_root: &Path) -> Option<()> {
    if shutdown_requested().load(Ordering::Relaxed) {
        return None;
    }
    let tmp_dir = cache_root.join(super::TEMP_DIR_NAME);
    fs::create_dir_all(&tmp_dir).ok()?;

    // ── Step 1: 4096×2048 preview TIF (input for steps 2 and 3) ─────────────
    let preview_tif = cache_root.join("gebco_2025_preview_4096.tif");
    if !preview_tif.exists() {
        let tmp_vrt = tmp_dir.join("gebco_derived.tmp.vrt");
        let mut buildvrt = Command::new(gdal_tool_path("gdalbuildvrt"));
        buildvrt.arg("-q").arg(&tmp_vrt);
        for tile in tiles {
            buildvrt.arg(tile);
        }
        run_command_with_timeout(
            buildvrt,
            "gdalbuildvrt (GEBCO preview)",
            Duration::from_secs(120),
        )
        .ok()?;

        if shutdown_requested().load(Ordering::Relaxed) {
            let _ = fs::remove_file(&tmp_vrt);
            return None;
        }

        let tmp_tif = tmp_dir.join("gebco_preview.tmp.tif");
        let mut translate = Command::new(gdal_tool_path("gdal_translate"));
        translate.args([
            "-q", "-outsize", "4096", "2048", "-ot", "Int16", "-of", "GTiff",
        ]);
        translate.arg(&tmp_vrt).arg(&tmp_tif);
        run_command_with_timeout(
            translate,
            "gdal_translate (GEBCO preview)",
            Duration::from_secs(300),
        )
        .ok()?;
        let _ = fs::remove_file(&tmp_vrt);

        if shutdown_requested().load(Ordering::Relaxed) {
            let _ = fs::remove_file(&tmp_tif);
            return None;
        }

        if fs::rename(&tmp_tif, &preview_tif).is_err() {
            fs::copy(&tmp_tif, &preview_tif).ok()?;
            let _ = fs::remove_file(&tmp_tif);
        }
    }

    // ── Step 2: 1440×720 depth BIL (globe depth-fill texture) ────────────────
    let depth_bil = cache_root.join("gebco_depth_1440x720.bil");
    if !depth_bil.exists() {
        let tmp_bil = tmp_dir.join("gebco_depth.tmp.bil");
        let mut translate = Command::new(gdal_tool_path("gdal_translate"));
        translate.args([
            "-q", "-outsize", "1440", "720", "-ot", "Int16", "-of", "EHdr",
        ]);
        translate.arg(&preview_tif).arg(&tmp_bil);
        run_command_with_timeout(
            translate,
            "gdal_translate (GEBCO depth BIL)",
            Duration::from_secs(60),
        )
        .ok()?;

        if shutdown_requested().load(Ordering::Relaxed) {
            let _ = fs::remove_file(&tmp_bil);
            return None;
        }

        // EHdr driver writes a companion .hdr; rename both.
        let tmp_hdr = tmp_bil.with_extension("hdr");
        let out_hdr = depth_bil.with_extension("hdr");
        if fs::rename(&tmp_bil, &depth_bil).is_err() {
            fs::copy(&tmp_bil, &depth_bil).ok()?;
            let _ = fs::remove_file(&tmp_bil);
        }
        if tmp_hdr.exists() {
            if fs::rename(&tmp_hdr, &out_hdr).is_err() {
                let _ = fs::copy(&tmp_hdr, &out_hdr);
                let _ = fs::remove_file(&tmp_hdr);
            }
        }
    }

    // ── Step 3: 200 m bathymetry contour GeoPackage ───────────────────────────
    let contours_gpkg = cache_root.join("gebco_2025_contours_200m.gpkg");
    if !contours_gpkg.exists() {
        let tmp_gpkg = tmp_dir.join("gebco_contours.tmp.gpkg");
        let mut contour = Command::new(gdal_tool_path("gdal_contour"));
        contour.args([
            "-q",
            "-f",
            "GPKG",
            "-a",
            "elevation_m",
            "-i",
            "200",
            "-snodata",
            "-32768",
            "-nln",
            "contour",
        ]);
        contour.arg(&preview_tif).arg(&tmp_gpkg);
        run_command_with_timeout(
            contour,
            "gdal_contour (GEBCO 200 m)",
            Duration::from_secs(600),
        )
        .ok()?;

        if shutdown_requested().load(Ordering::Relaxed) {
            let _ = fs::remove_file(&tmp_gpkg);
            return None;
        }

        if fs::rename(&tmp_gpkg, &contours_gpkg).is_err() {
            fs::copy(&tmp_gpkg, &contours_gpkg).ok()?;
            let _ = fs::remove_file(&tmp_gpkg);
        }
    }

    Some(())
}

pub fn lunar_preview_building() -> &'static AtomicBool {
    static BUILDING: OnceLock<AtomicBool> = OnceLock::new();
    BUILDING.get_or_init(|| AtomicBool::new(false))
}

// ── Mars spatial index ────────────────────────────────────────────────────────
//
// Every CTX DTM lives in its own orthographic projection centred on the stereo
// pair.  `gdalbuildvrt` cannot mosaic files with incompatible CRS, and opening
// 44 k files to build a warped VRT would take tens of minutes.
//
// Instead we parse the lat/lon centre of each DTM from its directory name
// (the last underscore-separated token encodes it, e.g. "04S063W"),
// cache the resulting index in a static, and for each contour-tile build we
// query that index to find the handful of source tiles that overlap the
// requested bounding box.  `gdalwarp` then reprojects those tiles on the fly
// into Mars longlat.

#[derive(Clone)]
struct MarsIndexEntry {
    lat: f32,
    lon: f32, // east, –180 … +180
    dem_path: PathBuf,
}

fn mars_tile_index() -> &'static Mutex<Option<(PathBuf, Vec<MarsIndexEntry>)>> {
    static INDEX: OnceLock<Mutex<Option<(PathBuf, Vec<MarsIndexEntry>)>>> = OnceLock::new();
    INDEX.get_or_init(|| Mutex::new(None))
}

/// Parse the approximate lat/lon centre from a CTX DTM directory name.
///
/// Directory names follow the pattern:
///   `<img1_name>__<img2_name>`
/// where each image name ends with a lat/lon suffix like `04S063W` (7 chars):
///   - 2-digit latitude, hemisphere letter (N/S)
///   - 3-digit *west* longitude, hemisphere letter (E/W)
fn parse_ctx_center(dir_name: &str) -> Option<(f32, f32)> {
    // Use the first image name (before `__`).
    let first = dir_name.split("__").next()?;
    // The lat/lon suffix is the last `_`-delimited token.
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

/// Walk `<data_root>/mars_data/` and build the spatial index.
/// Each sub-directory is named after its CTX image pair; we parse the
/// lat/lon from the name and record the path to `*-DEM-geoid-adj.tif`.
fn build_mars_tile_index(data_root: &Path) -> Vec<MarsIndexEntry> {
    let mars_data = data_root.join("mars_data");
    let Ok(top_entries) = fs::read_dir(&mars_data) else {
        return Vec::new();
    };
    let mut index = Vec::new();
    for entry in top_entries.flatten() {
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
        // The elevation file we want is `<dir_name>-DEM-geoid-adj.tif`.
        let dem_path = dir_path.join(format!("{dir_name}-DEM-geoid-adj.tif"));
        if dem_path.exists() {
            index.push(MarsIndexEntry { lat, lon, dem_path });
        }
    }
    index
}

/// Return the `*-DEM-geoid-adj.tif` files whose centre falls within
/// `bounds` expanded by `SPATIAL_BUFFER_DEG` on every side.
/// The first call scans `<data_root>/mars_data/` to build the index; all
/// subsequent calls reuse the cached index (invalidated on `data_root` change).
pub fn find_mars_tiles_for_bounds(data_root: &Path, bounds: GeoBounds) -> Vec<PathBuf> {
    // Buffer to account for along-track extent of individual DTMs (~100–300 km).
    const SPATIAL_BUFFER_DEG: f32 = 3.0;

    let guard = mars_tile_index().lock();
    let Ok(mut guard) = guard else {
        return Vec::new();
    };

    // Build or invalidate the cached index.
    let data_root_buf = data_root.to_path_buf();
    if guard.as_ref().map(|(root, _)| root != &data_root_buf).unwrap_or(true) {
        *guard = Some((data_root_buf, build_mars_tile_index(data_root)));
    }

    let index = match guard.as_ref() {
        Some((_, idx)) => idx,
        None => return Vec::new(),
    };

    let min_lat = bounds.min_lat - SPATIAL_BUFFER_DEG;
    let max_lat = bounds.max_lat + SPATIAL_BUFFER_DEG;
    let min_lon = bounds.min_lon - SPATIAL_BUFFER_DEG;
    let max_lon = bounds.max_lon + SPATIAL_BUFFER_DEG;

    index
        .iter()
        .filter(|e| e.lat >= min_lat && e.lat <= max_lat && e.lon >= min_lon && e.lon <= max_lon)
        .map(|e| e.dem_path.clone())
        .collect()
}

/// Build the SLDEM2015 lunar terrain preview PNG into `cache_root`
/// (= `Derived/terrain/`).
///
/// Output: `sldem2015_preview_4096.png` — 4096×1366 UInt16 PNG.
/// The PNG is scaled so that raw JP2 DN value -18000 → u16 0 and
/// +22000 → u16 65535, mapping to elevation_m = -9000 m … +11000 m.
/// (Actual data range: DN -17438…+21567, i.e. -8719 m … +10783 m.)
/// Coverage: 60°S to 60°N (the full SLDEM2015 extent).
///
/// Skipped if the output already exists.
pub fn build_lunar_preview(jp2_path: &Path, cache_root: &Path) -> Option<()> {
    if shutdown_requested().load(Ordering::Relaxed) {
        return None;
    }
    let out_png = cache_root.join("sldem2015_preview_4096.png");
    if out_png.exists() {
        return Some(());
    }

    let tmp_dir = cache_root.join(super::TEMP_DIR_NAME);
    fs::create_dir_all(&tmp_dir).ok()?;
    let tmp_png = tmp_dir.join("sldem2015_preview.tmp.png");

    // gdal_translate: downsample to 4096×1366, scale DN range [-18000, 22000]
    // to UInt16 [0, 65535], output as PNG (lossless 16-bit).
    // 4096 wide × 1366 tall is proportional to 360°×120° at 4096px wide.
    // Actual data min/max DN: -17438 … +21567 — use -18000/+22000 for headroom.
    let mut translate = Command::new(gdal_tool_path("gdal_translate"));
    translate.args([
        "-q", "-outsize", "4096", "1366", "-ot", "UInt16", "-of", "PNG", "-scale", "-18000",
        "22000", "0", "65535",
    ]);
    translate.arg(jp2_path).arg(&tmp_png);
    run_command_with_timeout(
        translate,
        "gdal_translate (SLDEM2015 lunar preview)",
        Duration::from_secs(300),
    )
    .ok()?;

    if shutdown_requested().load(Ordering::Relaxed) {
        let _ = fs::remove_file(&tmp_png);
        return None;
    }

    if fs::rename(&tmp_png, &out_png).is_err() {
        fs::copy(&tmp_png, &out_png).ok()?;
        let _ = fs::remove_file(&tmp_png);
    }

    Some(())
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

#[derive(Clone)]
struct LunarSourceChunk {
    path: PathBuf,
    bounds: GeoBounds,
    raster_size: u32,
}

fn lunar_source_chunk_root(cache_root: &Path) -> PathBuf {
    cache_root.join(LUNAR_SOURCE_CHUNK_DIR)
}

fn lunar_source_chunk_for_bounds(
    cache_root: &Path,
    bounds: GeoBounds,
    spec: FocusContourSpec,
) -> LunarSourceChunk {
    let center_lat = (bounds.min_lat + bounds.max_lat) * 0.5;
    let center_lon = (bounds.min_lon + bounds.max_lon) * 0.5;
    let lat_bucket = (center_lat / LUNAR_SOURCE_CHUNK_CENTER_STEP_DEG).round() as i32;
    let lon_bucket = (center_lon / LUNAR_SOURCE_CHUNK_CENTER_STEP_DEG).round() as i32;
    let chunk_center_lat = lat_bucket as f32 * LUNAR_SOURCE_CHUNK_CENTER_STEP_DEG;
    let chunk_center_lon = lon_bucket as f32 * LUNAR_SOURCE_CHUNK_CENTER_STEP_DEG;
    let pixels_per_degree = spec.raster_size as f32 / (spec.half_extent_deg * 2.0);
    let chunk_span = LUNAR_SOURCE_CHUNK_HALF_EXTENT_DEG * 2.0;
    let raster_size = (chunk_span * pixels_per_degree).ceil() as u32;
    let dir = lunar_source_chunk_root(cache_root).join(format!("z{}", spec.zoom_bucket));
    let file_name = format!("lat{lat_bucket:+04}_lon{lon_bucket:+04}.tif");
    LunarSourceChunk {
        path: dir.join(file_name),
        bounds: GeoBounds {
            min_lat: (chunk_center_lat - LUNAR_SOURCE_CHUNK_HALF_EXTENT_DEG).clamp(-89.999, 89.999),
            max_lat: (chunk_center_lat + LUNAR_SOURCE_CHUNK_HALF_EXTENT_DEG).clamp(-89.999, 89.999),
            min_lon: chunk_center_lon - LUNAR_SOURCE_CHUNK_HALF_EXTENT_DEG,
            max_lon: chunk_center_lon + LUNAR_SOURCE_CHUNK_HALF_EXTENT_DEG,
        },
        raster_size: raster_size.max(spec.raster_size),
    }
}

fn temp_sibling(path: &Path, suffix: &str) -> PathBuf {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("chunk");
    path.with_file_name(format!("{stem}.{suffix}"))
}

fn unique_temp_token() -> u64 {
    static NEXT: OnceLock<AtomicU64> = OnceLock::new();
    NEXT.get_or_init(|| AtomicU64::new(1))
        .fetch_add(1, Ordering::Relaxed)
}

fn lunar_source_chunk_pending() -> &'static Mutex<HashSet<PathBuf>> {
    static PENDING: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    PENDING.get_or_init(|| Mutex::new(HashSet::new()))
}

fn wait_for_lunar_source_chunk(path: &Path, timeout: Duration) -> bool {
    let start = Instant::now();
    loop {
        if path.exists() {
            return true;
        }
        if shutdown_requested().load(Ordering::Relaxed) {
            return false;
        }
        let still_pending = lunar_source_chunk_pending()
            .lock()
            .map(|guard| guard.contains(path))
            .unwrap_or(false);
        if !still_pending {
            return path.exists();
        }
        if start.elapsed() >= timeout {
            return path.exists();
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn persist_temp_file(tmp_path: &Path, final_path: &Path) -> std::io::Result<()> {
    if fs::rename(tmp_path, final_path).is_ok() {
        return Ok(());
    }
    fs::copy(tmp_path, final_path)?;
    fs::remove_file(tmp_path)?;
    Ok(())
}

fn ensure_lunar_source_chunk(
    jp2_path: &Path,
    cache_root: &Path,
    bounds: GeoBounds,
    spec: FocusContourSpec,
) -> Option<LunarSourceChunk> {
    let chunk = lunar_source_chunk_for_bounds(cache_root, bounds, spec);
    if chunk.path.exists() {
        return Some(chunk);
    }

    {
        let mut guard = lunar_source_chunk_pending().lock().ok()?;
        if !guard.insert(chunk.path.clone()) {
            drop(guard);
            return wait_for_lunar_source_chunk(&chunk.path, Duration::from_secs(120))
                .then_some(chunk);
        }
    }

    let result = (|| {
        let parent = chunk.path.parent()?;
        fs::create_dir_all(parent).ok()?;

        let tmp_chunk = temp_sibling(
            &chunk.path,
            &format!("{}.{}.tmp.tif", std::process::id(), unique_temp_token()),
        );
        let _ = fs::remove_file(&tmp_chunk);

        let mut translate = Command::new(gdal_tool_path("gdal_translate"));
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
            "-a_nodata",
            "-32768",
            "-ot",
            "Int16",
            "-r",
            "bilinear",
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
        run_command_with_timeout(
            translate,
            "gdal_translate (lunar source chunk)",
            Duration::from_secs(600),
        )
        .ok()?;
        persist_temp_file(&tmp_chunk, &chunk.path).ok()?;
        Some(())
    })();

    if let Ok(mut guard) = lunar_source_chunk_pending().lock() {
        guard.remove(&chunk.path);
    }

    result?;
    Some(chunk)
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

    run_gdal_contour(&tmp_tif_path, &tmp_gpkg_path, spec.interval_m, Some(-32768)).ok()?;
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

/// Build one lunar (SLDEM2015) contour tile.
///
/// Unlike the SRTM pipeline (which mosaics many 1°×1° tiles with gdalwarp),
/// SLDEM2015 is a single JP2 file. We use `gdal_translate -projwin` to extract
/// the geographic bounding box and scale raw Int16 DN values to Float32 elevation
/// in metres (DN × 0.5 = elevation_m, encoded as -scale -18000 22000 -9000 11000),
/// then run `gdal_contour` on that Float32 GeoTIFF.
pub fn build_lunar_contour_tile(
    jp2_path: &Path,
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

    if let Some(parent) = tmp_tif_path.parent() {
        fs::create_dir_all(parent).ok()?;
    }

    let chunk = ensure_lunar_source_chunk(jp2_path, cache_root, bounds, spec)?;

    // gdal_translate: crop from the cached source chunk into the exact tile.
    // -projwin ulx uly lrx lry  (min_lon, max_lat, max_lon, min_lat)
    let mut translate = Command::new(gdal_tool_path("gdal_translate"));
    translate.args([
        "-q",
        "-projwin",
        &bounds.min_lon.to_string(),
        &bounds.max_lat.to_string(),
        &bounds.max_lon.to_string(),
        &bounds.min_lat.to_string(),
        "-outsize",
        &spec.raster_size.to_string(),
        &spec.raster_size.to_string(),
        "-a_nodata",
        "-32768",
        "-ot",
        "Int16",
        "-r",
        "bilinear",
        "-of",
        "GTiff",
    ]);
    translate.arg(&chunk.path).arg(&tmp_tif_path);
    run_command_with_timeout(
        translate,
        "gdal_translate (lunar cached tile)",
        Duration::from_secs(120),
    )
    .ok()?;

    if shutdown_requested().load(Ordering::Relaxed) {
        cleanup_temp_tile_artifacts(&tmp_tif_path, &tmp_gpkg_path);
        return None;
    }

    run_gdal_contour(&tmp_tif_path, &tmp_gpkg_path, spec.interval_m, Some(-32768)).ok()?;
    import_tile_into_cache(cache_db_path, tile, &tmp_gpkg_path).ok()?;
    cleanup_temp_tile_artifacts(&tmp_tif_path, &tmp_gpkg_path);
    Some(())
}

/// Build a Mars contour tile by:
///   1. Querying the spatial index for source `*-DEM-geoid-adj.tif` files whose
///      centre falls within a generous buffer around `bounds`.
///   2. Warping the matching tiles (each in its own orthographic CRS) into Mars
///      longlat using `gdalwarp`, clipping to `bounds`.
///   3. Running `gdal_contour` on the result.
///
/// If no source tiles overlap the region the tile is marked empty in the cache
/// (stored with contour_count = 0) so the region is not retried every frame.
pub fn build_mars_contour_tile(
    data_root: &Path,
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

    if let Some(parent) = tmp_tif_path.parent() {
        fs::create_dir_all(parent).ok()?;
    }

    // Find source tiles that cover this bounding box.
    let source_tiles = find_mars_tiles_for_bounds(data_root, bounds);

    if source_tiles.is_empty() {
        // No CTX coverage here — store an empty tile so we don't retry.
        mark_tile_empty(cache_db_path, tile).ok()?;
        return Some(());
    }

    if shutdown_requested().load(Ordering::Relaxed) {
        return None;
    }

    // gdalwarp: reproject all source tiles (each in unique orthographic CRS)
    // into Mars longlat, clipping to the tile bounding box.
    let mut warp = Command::new(gdal_tool_path("gdalwarp"));
    warp.args([
        "-q",
        "-overwrite",
        "-t_srs",
        "+proj=longlat +R=3396190 +no_defs",
        "-r",
        "bilinear",
        "-dstnodata",
        "-32767",
        "-te",
        &format!("{:.6}", bounds.min_lon),
        &format!("{:.6}", bounds.min_lat),
        &format!("{:.6}", bounds.max_lon),
        &format!("{:.6}", bounds.max_lat),
        "-ts",
        &spec.raster_size.to_string(),
        &spec.raster_size.to_string(),
    ]);
    for tile_path in &source_tiles {
        warp.arg(tile_path);
    }
    warp.arg(&tmp_tif_path);
    run_command_with_timeout(warp, "gdalwarp (mars ctx tile)", Duration::from_secs(180)).ok()?;

    if shutdown_requested().load(Ordering::Relaxed) {
        cleanup_temp_tile_artifacts(&tmp_tif_path, &tmp_gpkg_path);
        return None;
    }

    run_gdal_contour(&tmp_tif_path, &tmp_gpkg_path, spec.interval_m, Some(-32767)).ok()?;
    import_tile_into_cache(cache_db_path, tile, &tmp_gpkg_path).ok()?;
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

fn run_gdal_contour(input_path: &Path, output_path: &Path, interval_m: i32, nodata: Option<i32>) -> std::io::Result<()> {
    let mut command = Command::new(gdal_tool_path("gdal_contour"));
    command.args([
        "-q",
        "-f",
        "GPKG",
        "-a",
        "elevation_m",
        "-i",
        &interval_m.to_string(),
    ]);
    if let Some(nd) = nodata {
        command.args(["-snodata", &nd.to_string()]);
    }
    command.args([
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
