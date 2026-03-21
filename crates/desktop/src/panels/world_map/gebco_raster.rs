//! Direct GEBCO 2025 raster sampling for bathymetric depth visualisation.
//!
//! The GPKG-based bathymetry used `gdal_contour` output, which fragments every
//! isobath into thousands of short scan-line segments — requiring sort-by-length
//! heuristics that still left spotty coverage.  This module reads the raw
//! elevation GeoTIFFs directly: a 1440×720 downsampled depth grid (pre-generated
//! once from the 8 source tiles) is loaded at startup and sampled at a regular
//! lat/lon grid.  Each ocean pixel becomes one coloured quad in a single egui
//! Mesh — zero fragmentation, complete global coverage.
//!
//! # File layout
//! ```text
//! {derived_root}/terrain/gebco_2025.vrt              (VRT mosaic, optional)
//! {derived_root}/terrain/gebco_2025_depth_1440x720.tif  (depth grid, required)
//! ```
//! The depth grid is generated automatically in a background thread the first
//! time the module loads if the raw tiles are present but the grid is not.
//!
//! # LOD
//! Two pre-filtered point lists are computed once from the depth grid:
//!   - **coarse** (step=6): ~20 000 ocean points — used at globe zoom < 2
//!   - **fine**   (step=3): ~80 000 ocean points — used at globe zoom ≥ 2

use crate::settings_store;
use crate::terrain_assets;
use image::ImageReader;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, OnceLock};

// ── constants ─────────────────────────────────────────────────────────────────

const GRID_W: u32 = 1440;
const GRID_H: u32 = 720;
const GRID_FILENAME: &str = "gebco_2025_depth_1440x720.tif";
const VRT_FILENAME: &str = "gebco_2025.vrt";
/// GEBCO nodata sentinel value (Int16 -32767).
const NODATA: i16 = -32767;

// ── depth grid ────────────────────────────────────────────────────────────────

pub struct GebcoDepthGrid {
    /// Int16 elevation samples, row-major, top row = +90° latitude.
    pixels: Vec<i16>,
}

impl GebcoDepthGrid {
    /// Returns `(lat, lon, depth_m)` tuples for every ocean pixel (elevation < 0)
    /// sampled at the given stride.  Pre-computed once at load time.
    pub fn ocean_points(&self, step: u32) -> Vec<(f32, f32, f32)> {
        let step = step.max(1) as usize;
        let mut out = Vec::with_capacity(100_000);
        for row in (0..GRID_H as usize).step_by(step) {
            for col in (0..GRID_W as usize).step_by(step) {
                let v = self.pixels[row * GRID_W as usize + col];
                if v >= 0 || v == NODATA {
                    continue; // land or nodata
                }
                // Pixel centre coordinates
                let lat = 90.0 - (row as f32 + 0.5) * (180.0 / GRID_H as f32);
                let lon = -180.0 + (col as f32 + 0.5) * (360.0 / GRID_W as f32);
                out.push((lat, lon, v as f32));
            }
        }
        out
    }
}

// ── cache ─────────────────────────────────────────────────────────────────────

struct DepthCache {
    /// Pre-filtered ocean points at step=6 (~20K items).
    coarse: Arc<Vec<(f32, f32, f32)>>,
    /// Pre-filtered ocean points at step=3 (~80K items).
    fine: Arc<Vec<(f32, f32, f32)>>,
}

static DEPTH_CACHE: OnceLock<Mutex<Option<Arc<DepthCache>>>> = OnceLock::new();
/// Guards against spawning the generation thread more than once.
static GENERATING: OnceLock<Mutex<bool>> = OnceLock::new();

// ── public API ────────────────────────────────────────────────────────────────

/// Returns `(coarse_points, fine_points)` where each is a pre-filtered,
/// pre-allocated slice of `(lat, lon, depth_m)` ocean samples.
///
/// Returns `None` if the depth grid has not been generated yet — the function
/// triggers background generation automatically when the raw tiles are present.
pub fn load_depth_grid(
    selected_root: Option<&Path>,
) -> Option<(Arc<Vec<(f32, f32, f32)>>, Arc<Vec<(f32, f32, f32)>>)> {
    let cache = DEPTH_CACHE.get_or_init(|| Mutex::new(None));
    let mut guard = cache.lock().ok()?;

    if let Some(cached) = guard.as_ref() {
        return Some((Arc::clone(&cached.coarse), Arc::clone(&cached.fine)));
    }

    let path = depth_grid_path(selected_root)?;
    if !path.exists() {
        // Try to generate in background if raw tiles are available.
        ensure_generated(selected_root);
        return None;
    }

    let grid = load_depth_grid_file(&path)?;
    let coarse = Arc::new(grid.ocean_points(6));
    let fine = Arc::new(grid.ocean_points(3));
    let dc = Arc::new(DepthCache {
        coarse: Arc::clone(&coarse),
        fine: Arc::clone(&fine),
    });
    *guard = Some(dc);
    Some((coarse, fine))
}

/// Clear cached ocean-point data — called by the global Cache Blast action.
pub fn blast_cache() {
    if let Some(cache) = DEPTH_CACHE.get() {
        if let Ok(mut g) = cache.lock() {
            *g = None;
        }
    }
}

// ── path helpers ──────────────────────────────────────────────────────────────

fn depth_grid_path(selected_root: Option<&Path>) -> Option<PathBuf> {
    let derived = terrain_assets::find_derived_root(selected_root)?;
    Some(derived.join("terrain").join(GRID_FILENAME))
}

fn vrt_path(selected_root: Option<&Path>) -> Option<PathBuf> {
    let derived = terrain_assets::find_derived_root(selected_root)?;
    Some(derived.join("terrain").join(VRT_FILENAME))
}

/// Locate the directory containing the raw GEBCO 2025 GeoTIFF tiles.
/// Scans `{data_root}/GEBCO_*/` directories for the expected file pattern.
pub fn find_gebco_raw_dir(selected_root: Option<&Path>) -> Option<PathBuf> {
    let data_root = terrain_assets::find_data_root(selected_root)?;
    let dir = std::fs::read_dir(&data_root).ok()?;
    for entry in dir.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.starts_with("GEBCO") {
            // Probe for one of the expected tile names
            let probe =
                path.join("gebco_2025_n90.0_s0.0_w-180.0_e-120.0_geotiff.tif");
            if probe.exists() {
                return Some(path);
            }
        }
    }
    None
}

// ── file loading ──────────────────────────────────────────────────────────────

fn load_depth_grid_file(path: &Path) -> Option<GebcoDepthGrid> {
    // Fast path: image crate handles simple 16-bit TIFFs.
    if let Some(grid) = load_via_image(path) {
        return Some(grid);
    }
    // Slow path: some GeoTIFF variants (Int16 TIFF with GeoTIFF metadata tags)
    // cause the image crate to fail silently.  Convert to a headerless raw
    // Int16 binary (EHdr / BIL format) via gdal_translate once, then read
    // directly — exactly the same two-stage approach used by srtm_stream.rs.
    load_via_gdal_convert(path)
}

fn load_via_image(path: &Path) -> Option<GebcoDepthGrid> {
    let image = ImageReader::open(path).ok()?.decode().ok()?.to_luma16();
    let (w, h) = image.dimensions();
    if w != GRID_W || h != GRID_H {
        return None;
    }
    let pixels: Vec<i16> = image.into_raw().into_iter().map(|u| u as i16).collect();
    // Sanity check: a successfully decoded ocean depth file must have negative
    // values.  If all values are ≥ 0 the sign-extension was lost during decode.
    if pixels.iter().all(|&v| v >= 0) {
        return None;
    }
    Some(GebcoDepthGrid { pixels })
}

/// Convert the GeoTIFF to a headerless raw Int16 BIL file via gdal_translate
/// (cached next to the source with a `.bil` extension), then read it.
fn load_via_gdal_convert(tif_path: &Path) -> Option<GebcoDepthGrid> {
    let bil_path = tif_path.with_extension("bil");
    if !bil_path.exists() {
        // Pass the full .bil path to gdal_translate so it names the output file
        // explicitly — passing the bare stem causes GDAL to emit an extensionless
        // file on macOS, which we would then fail to find.
        let gdal_translate = settings_store::resolve_gdal_tool("gdal_translate");
        let ok = std::process::Command::new(&gdal_translate)
            .args(["-q", "-ot", "Int16", "-of", "EHdr"])
            .arg(tif_path)
            .arg(&bil_path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ok {
            eprintln!("gebco_raster: gdal_translate fallback failed for {:?}", tif_path);
            return None;
        }
    }
    load_raw_int16_le(&bil_path)
}

/// Read a headerless little-endian Int16 binary file of exactly GRID_W × GRID_H
/// samples.  This is what `gdal_translate -of EHdr` produces.
fn load_raw_int16_le(path: &Path) -> Option<GebcoDepthGrid> {
    let bytes = std::fs::read(path).ok()?;
    let expected = GRID_W as usize * GRID_H as usize * 2;
    if bytes.len() != expected {
        eprintln!(
            "gebco_raster: raw file is {} bytes, expected {}",
            bytes.len(),
            expected
        );
        return None;
    }
    let pixels: Vec<i16> = bytes
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]))
        .collect();
    Some(GebcoDepthGrid { pixels })
}

// ── background generation ─────────────────────────────────────────────────────

/// Spawn a background thread that builds the VRT mosaic and downsampled depth
/// grid from the raw GEBCO tile files.  Does nothing if already generating or
/// if the raw tiles cannot be found.
fn ensure_generated(selected_root: Option<&Path>) {
    let gen_lock = GENERATING.get_or_init(|| Mutex::new(false));
    let mut generating = gen_lock.lock().unwrap();
    if *generating {
        return;
    }

    let Some(tiles_dir) = find_gebco_raw_dir(selected_root) else { return };
    let Some(vrt) = vrt_path(selected_root) else { return };
    let Some(output) = depth_grid_path(selected_root) else { return };

    *generating = true;
    drop(generating);

    let gdalbuildvrt = settings_store::resolve_gdal_tool("gdalbuildvrt");
    let gdal_translate = settings_store::resolve_gdal_tool("gdal_translate");

    std::thread::spawn(move || {
        // Collect the 8 main elevation tiles (exclude sub_ice and tid variants).
        let tile_files: Vec<PathBuf> = std::fs::read_dir(&tiles_dir)
            .ok()
            .into_iter()
            .flat_map(|d| d.flatten())
            .map(|e| e.path())
            .filter(|p| {
                let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
                if ext != "tif" {
                    return false;
                }
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                name.starts_with("gebco_2025_")
                    && !name.contains("sub_ice")
                    && !name.contains("_tid_")
            })
            .collect();

        if tile_files.is_empty() {
            mark_done();
            return;
        }

        // Build VRT mosaic.
        let mut cmd = std::process::Command::new(&gdalbuildvrt);
        cmd.arg(&vrt);
        for tf in &tile_files {
            cmd.arg(tf);
        }
        if cmd
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_err()
        {
            mark_done();
            return;
        }

        // Downsample to 1440×720 (≈0.25° per pixel) with block averaging.
        let status = std::process::Command::new(&gdal_translate)
            .args([
                "-q", "-of", "GTiff", "-ot", "Int16",
                "-outsize", "1440", "720",
                "-r", "average",
            ])
            .arg(&vrt)
            .arg(&output)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();

        if status.map(|s| s.success()).unwrap_or(false) {
            // Clear the cache so the next render frame loads the new file.
            if let Some(cache) = DEPTH_CACHE.get() {
                if let Ok(mut g) = cache.lock() {
                    *g = None;
                }
            }
        }

        mark_done();
    });
}

fn mark_done() {
    if let Some(gen_lock) = GENERATING.get() {
        if let Ok(mut g) = gen_lock.lock() {
            *g = false;
        }
    }
}
