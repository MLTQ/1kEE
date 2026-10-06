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

// ── grid constants ────────────────────────────────────────────────────────────
const GRID_W: usize = 1440;
const GRID_H: usize = 720;
const NODATA: i16 = -32767;

type TextureCache =
    super::layer_snapshot::Snapshot<Option<std::path::PathBuf>, egui::TextureHandle>;
static TEXTURE: OnceLock<Mutex<TextureCache>> = OnceLock::new();

/// Request a background read/conversion and return the last completed texture.
pub fn ensure_texture(
    ctx: &egui::Context,
    selected_root: Option<&Path>,
) -> Option<egui::TextureId> {
    let wake = ctx.clone();
    let result = TEXTURE.get_or_init(Default::default).lock().ok()?.get(
        selected_root.map(Path::to_path_buf),
        "gebco-depth-image",
        |a, b| a == b,
        move |root| {
            let (path, _) = super::srtm_focus_cache::ensure_gebco_derived(root.as_deref());
            let bytes = std::fs::read(path?).ok()?;
            let image = color_image(&bytes)?;
            Some(wake.load_texture("gebco_depth_fill", image, egui::TextureOptions::LINEAR))
        },
    );
    if result.is_none() {
        ctx.request_repaint_after(std::time::Duration::from_secs(1));
    }
    result.map(|h| h.id())
}

pub fn clear() {
    if let Some(cache) = TEXTURE.get() {
        cache.lock().unwrap().clear();
    }
}

fn color_image(bytes: &[u8]) -> Option<egui::ColorImage> {
    if bytes.len() != GRID_W * GRID_H * 2 {
        return None;
    }
    let pixels = bytes
        .chunks_exact(2)
        .map(|c| {
            let v = i16::from_le_bytes([c[0], c[1]]);
            if v == NODATA || v >= 0 {
                egui::Color32::TRANSPARENT
            } else {
                depth_color(v)
            }
        })
        .collect();
    Some(egui::ColorImage {
        size: [GRID_W, GRID_H],
        pixels,
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
    let r = lerp(8.0, 1.0, t) as u8;
    let g = lerp(22.0, 3.0, t) as u8;
    let b = lerp(62.0, 6.0, t) as u8;
    let a = lerp(210.0, 250.0, t) as u8;
    egui::Color32::from_rgba_premultiplied(r, g, b, a)
}

#[inline]
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}
