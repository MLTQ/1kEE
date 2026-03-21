/// GEBCO depth-fill layer for the globe view.
///
/// Loads a 1440×720 (0.25° per pixel) depth grid from
/// `gebco_depth_1440x720.bil` (pre-generated once via gdal_translate).
/// Converts it to an egui texture where ocean pixels carry a depth-colour
/// and land/nodata pixels are fully transparent.
///
/// Rendering contract
/// ------------------
/// `ensure_texture(ctx, root)` uploads the texture on first call and
/// returns a stable `TextureId` on every subsequent call.
///
/// `globe_scene` then builds a 2°×2° UV-mapped sphere mesh that references
/// the texture.  GPU bilinear interpolation (`TextureOptions::LINEAR`)
/// gives smooth depth gradients that follow the actual bathymetry — no
/// rectangular grid artefacts, no land bleed.

use std::{
    path::Path,
    sync::{Mutex, OnceLock},
};

use crate::terrain_assets;

// ── grid constants ────────────────────────────────────────────────────────────
const GRID_W: usize = 1440;
const GRID_H: usize = 720;
const NODATA: i16 = -32767;

// ── statics ───────────────────────────────────────────────────────────────────
static DEPTH_GRID: OnceLock<Mutex<Option<Vec<i16>>>> = OnceLock::new();
static TEXTURE_HANDLE: OnceLock<Mutex<Option<egui::TextureHandle>>> = OnceLock::new();

// ── public API ────────────────────────────────────────────────────────────────

/// Ensure the texture is uploaded and return its id, or `None` if the .bil
/// file is not present yet.
pub fn ensure_texture(
    ctx: &egui::Context,
    selected_root: Option<&Path>,
) -> Option<egui::TextureId> {
    let handle_cell = TEXTURE_HANDLE.get_or_init(|| Mutex::new(None));
    {
        let guard = handle_cell.lock().ok()?;
        if let Some(h) = guard.as_ref() {
            return Some(h.id());
        }
    }
    // Not uploaded yet — try to load the grid and build the texture.
    ensure_grid_loaded(selected_root)?;
    let image = build_color_image()?;
    let handle = ctx.load_texture(
        "gebco_depth_fill",
        image,
        egui::TextureOptions {
            magnification: egui::TextureFilter::Linear,
            minification: egui::TextureFilter::Linear,
            wrap_mode: egui::TextureWrapMode::ClampToEdge,
            mipmap_mode: None,
        },
    );
    let id = handle.id();
    let mut guard = handle_cell.lock().ok()?;
    *guard = Some(handle);
    Some(id)
}

/// Clear all caches (Cache Blast button).
pub fn clear() {
    if let Some(c) = DEPTH_GRID.get() {
        if let Ok(mut g) = c.lock() { *g = None; }
    }
    if let Some(c) = TEXTURE_HANDLE.get() {
        if let Ok(mut g) = c.lock() { *g = None; }
    }
}

// ── private ───────────────────────────────────────────────────────────────────

fn bil_path(selected_root: Option<&Path>) -> Option<std::path::PathBuf> {
    let derived = terrain_assets::find_derived_root(selected_root)?;
    let p = derived.join("terrain/gebco_depth_1440x720.bil");
    p.exists().then_some(p)
}

fn ensure_grid_loaded(selected_root: Option<&Path>) -> Option<()> {
    let cache = DEPTH_GRID.get_or_init(|| Mutex::new(None));
    {
        let guard = cache.lock().ok()?;
        if guard.is_some() { return Some(()); }
    }
    let path = bil_path(selected_root)?;
    let bytes = std::fs::read(&path).ok()?;
    if bytes.len() != GRID_W * GRID_H * 2 {
        eprintln!(
            "gebco_depth_fill: expected {} bytes, got {}",
            GRID_W * GRID_H * 2, bytes.len()
        );
        return None;
    }
    // .hdr says BYTEORDER I (Intel / little-endian).
    let pixels: Vec<i16> = bytes
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]))
        .collect();
    let mut guard = cache.lock().ok()?;
    *guard = Some(pixels);
    Some(())
}

fn build_color_image() -> Option<egui::ColorImage> {
    let cache = DEPTH_GRID.get()?.lock().ok()?;
    let pixels = cache.as_ref()?;

    let colors: Vec<egui::Color32> = pixels
        .iter()
        .map(|&v| {
            if v == NODATA || v >= 0 {
                egui::Color32::TRANSPARENT // land → globe background shows through
            } else {
                depth_color(v)
            }
        })
        .collect();

    Some(egui::ColorImage {
        size: [GRID_W, GRID_H],
        pixels: colors,
    })
}

/// Convert a negative depth value (metres) to a premultiplied colour.
///
/// Ramp (all dark — ocean is not the main focus, depth cues are):
///   shelf  (−200 m)  → dark steel-blue   b ≈ 56
///   slope  (−1 000 m) → dim navy          b ≈ 28
///   abyss  (−4 000 m) → very dark indigo  b ≈ 12
///   hadal  (−9 000 m) → near-black        b ≈ 5
pub fn depth_color(depth_m: i16) -> egui::Color32 {
    let d = (-depth_m as f32).clamp(1.0, 11_000.0);
    // powf(0.35): concentrates perceptual variation in shallow zone
    let t = (d / 11_000.0).powf(0.35);
    let r = lerp(8.0,  1.0, t) as u8;
    let g = lerp(22.0, 3.0, t) as u8;
    let b = lerp(62.0, 6.0, t) as u8;
    let a = lerp(210.0, 250.0, t) as u8;
    egui::Color32::from_rgba_premultiplied(r, g, b, a)
}

#[inline]
fn lerp(a: f32, b: f32, t: f32) -> f32 { a + (b - a) * t }
