/// Depth-fill layer for the globe view.
///
/// Loads a tiny 180×90 (2° per pixel) depth grid from the pre-generated
/// `gebco_depth_180x90.bil` file and exposes it for per-frame quad rendering.
///
/// At 2° per cell, the projected cell width at globe radius ~350px is ~12px —
/// cells tile seamlessly with no gaps, giving a solid filled ocean background
/// rather than the dot-grid artefact produced by smaller, sparser samples.
///
/// Rendering contract:
///   `depth_at(lat, lon) -> Option<i16>` — returns `None` for land / nodata,
///   `Some(depth_m)` for ocean (depth_m < 0).
///
/// The .bil is raw little-endian Int16, row-major, top-to-bottom (north first),
/// left-to-right (west first), covering -90..90 lat and -180..180 lon.

use std::{
    path::Path,
    sync::{Mutex, OnceLock},
};

use crate::terrain_assets;

const GRID_W: usize = 180;
const GRID_H: usize = 90;
const NODATA: i16 = -32767;
const CELL_DEG: f32 = 2.0; // degrees per cell

/// Cached depth grid: 180×90 Int16 values, row-major north→south.
static DEPTH_GRID: OnceLock<Mutex<Option<Vec<i16>>>> = OnceLock::new();

/// Return the GEBCO depth (m, negative = ocean) at the given lat/lon,
/// or None if the cell is land, nodata, or the grid is not yet loaded.
pub fn depth_at(lat: f32, lon: f32) -> Option<i16> {
    let grid = DEPTH_GRID.get_or_init(|| Mutex::new(None));
    let guard = grid.lock().ok()?;
    let pixels = guard.as_ref()?;

    // Convert lat/lon to grid indices.
    // Grid: row 0 = 90°N, col 0 = 180°W.
    let col = ((lon + 180.0) / CELL_DEG) as usize;
    let row = ((90.0 - lat) / CELL_DEG) as usize;
    let col = col.min(GRID_W - 1);
    let row = row.min(GRID_H - 1);

    let v = pixels[row * GRID_W + col];
    if v == NODATA || v >= 0 {
        None // land or sea-level
    } else {
        Some(v)
    }
}

/// Load (or trigger load of) the depth grid.  Call once per frame from
/// `draw_global_bathymetry`; returns true once the grid is ready.
pub fn ensure_loaded(selected_root: Option<&Path>) -> bool {
    let cache = DEPTH_GRID.get_or_init(|| Mutex::new(None));
    {
        let guard = cache.lock().unwrap_or_else(|e| e.into_inner());
        if guard.is_some() {
            return true;
        }
    }
    // Try to load synchronously (32KB — negligible cost).
    if let Some(pixels) = load(selected_root) {
        let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());
        *guard = Some(pixels);
        true
    } else {
        false
    }
}

/// Clear the cached grid (used by Cache Blast).
pub fn clear() {
    if let Some(cache) = DEPTH_GRID.get() {
        if let Ok(mut g) = cache.lock() {
            *g = None;
        }
    }
}

// ── private ──────────────────────────────────────────────────────────────────

fn bil_path(selected_root: Option<&Path>) -> Option<std::path::PathBuf> {
    let derived = terrain_assets::find_derived_root(selected_root)?;
    let p = derived.join("terrain/gebco_depth_180x90.bil");
    p.exists().then_some(p)
}

fn load(selected_root: Option<&Path>) -> Option<Vec<i16>> {
    let path = bil_path(selected_root)?;
    let bytes = std::fs::read(&path).ok()?;
    let expected = GRID_W * GRID_H * 2;
    if bytes.len() != expected {
        eprintln!(
            "gebco_depth_fill: expected {} bytes, got {} in {:?}",
            expected,
            bytes.len(),
            path
        );
        return None;
    }
    // EHdr BIL is BYTEORDER M (big-endian) according to the .hdr.
    // gdal_translate writes MSB by default for EHdr.
    // Detect endianness from .hdr file; fall back to big-endian.
    let big_endian = hdr_is_big_endian(&path);
    let pixels: Vec<i16> = bytes
        .chunks_exact(2)
        .map(|c| {
            if big_endian {
                i16::from_be_bytes([c[0], c[1]])
            } else {
                i16::from_le_bytes([c[0], c[1]])
            }
        })
        .collect();
    Some(pixels)
}

fn hdr_is_big_endian(bil_path: &Path) -> bool {
    let hdr = bil_path.with_extension("hdr");
    if let Ok(text) = std::fs::read_to_string(hdr) {
        for line in text.lines() {
            let l = line.trim().to_uppercase();
            if l.starts_with("BYTEORDER") {
                return !l.contains('I'); // 'I' = Intel/little-endian; 'M' = big
            }
        }
    }
    true // GDAL EHdr default is big-endian
}
