//! Server-rendered 3DEP hillshade draped over the local terrain surface.
//!
//! The existing elevation fill already shades a 60x60 interpolated mesh. This
//! layer is a different thing: the image service renders a multidirectional
//! hillshade from its own 1 m source and returns it as an 8-bit image, so the
//! relief detail is far finer than the fill mesh can express. The texture is
//! draped on the same projected surface, which keeps it registered with the
//! contours and markers rather than floating as a flat overlay.

use crate::model::GeoPoint;
use crate::model::GlobeViewState;
use std::sync::{Mutex, OnceLock};

use super::projection::project_local;
use super::{ElevationSurface, LocalLayout, visual_half_extent_for_zoom};

/// Requested texture edge. 1024 is a ~700 KB PNG and resolves better than one
/// screen pixel per texel across the deep zoom range.
const TEXTURE_PX: u32 = 1024;
/// Drape grid resolution. The surface is smooth, so this only needs to be fine
/// enough that the projection's curvature does not show as faceting.
const DRAPE_CELLS: usize = 48;
/// Panning within this fraction of the view keeps the current texture instead
/// of refetching, so a slow drag does not stream a new image every frame.
const REUSE_FRACTION: f32 = 0.35;

#[derive(Clone, Copy, PartialEq)]
struct ShadeBounds {
    min_lat: f32,
    min_lon: f32,
    max_lat: f32,
    max_lon: f32,
}

impl ShadeBounds {
    fn around(center: GeoPoint, half_extent_deg: f32) -> Self {
        Self {
            min_lat: center.lat - half_extent_deg,
            min_lon: center.lon - half_extent_deg,
            max_lat: center.lat + half_extent_deg,
            max_lon: center.lon + half_extent_deg,
        }
    }

    fn contains_with_margin(&self, other: &ShadeBounds, fraction: f32) -> bool {
        let lat_margin = (self.max_lat - self.min_lat) * fraction;
        let lon_margin = (self.max_lon - self.min_lon) * fraction;
        other.min_lat >= self.min_lat - lat_margin
            && other.max_lat <= self.max_lat + lat_margin
            && other.min_lon >= self.min_lon - lon_margin
            && other.max_lon <= self.max_lon + lon_margin
    }
}

struct LoadedShade {
    bounds: ShadeBounds,
    texture: egui::TextureHandle,
}

#[derive(Default)]
struct ShadeState {
    loaded: Option<LoadedShade>,
    /// Bounds of an in-flight request, so a pan mid-fetch does not pile up
    /// duplicate downloads.
    fetching: Option<ShadeBounds>,
    /// Decoded image waiting to be uploaded on the UI thread.
    pending: Option<(ShadeBounds, egui::ColorImage)>,
}

fn state() -> &'static Mutex<ShadeState> {
    static STATE: OnceLock<Mutex<ShadeState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(ShadeState::default()))
}

/// Drop the cached texture. Wired to the same cache-reset control as the other
/// terrain layers.
pub(crate) fn clear() {
    if let Ok(mut guard) = state().lock() {
        *guard = ShadeState::default();
    }
}

/// Draw the hillshade for the current view, fetching it in the background when
/// the view has moved beyond the cached image.
///
/// `surface` is the elevation field the fill mesh was built from; using it
/// keeps the drape on exactly the triangles already on screen. Without it the
/// texture is draped flat, which still reads correctly from the oblique camera.
pub(super) fn draw_hillshade(
    painter: &egui::Painter,
    layout: &LocalLayout,
    view: &GlobeViewState,
    focus: GeoPoint,
    surface: Option<&ElevationSurface>,
    extent_x_km: f32,
    extent_y_km: f32,
) {
    let half_extent_deg = visual_half_extent_for_zoom(view.local_zoom);
    let wanted = ShadeBounds::around(focus, half_extent_deg);

    upload_pending(painter.ctx());
    ensure_requested(painter.ctx(), wanted);

    let Ok(guard) = state().lock() else {
        return;
    };
    let Some(loaded) = guard.loaded.as_ref() else {
        return;
    };
    // A stale texture from a nearby view is better than a hole while the next
    // one downloads, but one from a wildly different area is not.
    if !loaded.bounds.contains_with_margin(&wanted, REUSE_FRACTION) {
        return;
    }
    let bounds = loaded.bounds;
    let texture_id = loaded.texture.id();
    drop(guard);

    let mut mesh = egui::Mesh::with_texture(texture_id);
    let mut indices: Vec<Option<u32>> = Vec::with_capacity((DRAPE_CELLS + 1).pow(2));

    for row in 0..=DRAPE_CELLS {
        let v = row as f32 / DRAPE_CELLS as f32;
        // Texture rows run north to south.
        let lat = bounds.max_lat - (bounds.max_lat - bounds.min_lat) * v;
        for col in 0..=DRAPE_CELLS {
            let u = col as f32 / DRAPE_CELLS as f32;
            let lon = bounds.min_lon + (bounds.max_lon - bounds.min_lon) * u;
            let point = GeoPoint { lat, lon };
            let elevation_m = surface.and_then(|s| s.elevation_at(point)).unwrap_or(0.0);
            match project_local(
                layout,
                view,
                focus,
                point,
                elevation_m,
                extent_x_km,
                extent_y_km,
            ) {
                Some(projected) => {
                    indices.push(Some(mesh.vertices.len() as u32));
                    mesh.vertices.push(egui::epaint::Vertex {
                        pos: projected.pos,
                        uv: egui::pos2(u, v),
                        color: egui::Color32::WHITE,
                    });
                }
                // Behind the camera or otherwise unprojectable: the cells that
                // touch this vertex are simply not emitted.
                None => indices.push(None),
            }
        }
    }

    let stride = DRAPE_CELLS + 1;
    for row in 0..DRAPE_CELLS {
        for col in 0..DRAPE_CELLS {
            let (Some(top_left), Some(top_right), Some(bottom_left), Some(bottom_right)) = (
                indices[row * stride + col],
                indices[row * stride + col + 1],
                indices[(row + 1) * stride + col],
                indices[(row + 1) * stride + col + 1],
            ) else {
                continue;
            };
            mesh.add_triangle(top_left, top_right, bottom_left);
            mesh.add_triangle(top_right, bottom_right, bottom_left);
        }
    }

    if !mesh.indices.is_empty() {
        painter.add(egui::Shape::mesh(mesh));
    }
}

/// Move a background-decoded image into a GPU texture. Texture upload needs the
/// egui context, so it cannot happen on the fetch thread.
fn upload_pending(ctx: &egui::Context) {
    let Ok(mut guard) = state().lock() else {
        return;
    };
    let Some((bounds, image)) = guard.pending.take() else {
        return;
    };
    let texture = ctx.load_texture(
        "threedep_hillshade",
        image,
        egui::TextureOptions {
            magnification: egui::TextureFilter::Linear,
            minification: egui::TextureFilter::Linear,
            wrap_mode: egui::TextureWrapMode::ClampToEdge,
            mipmap_mode: None,
        },
    );
    guard.loaded = Some(LoadedShade { bounds, texture });
}

/// Schedule a fetch when the cached texture no longer covers the view.
fn ensure_requested(ctx: &egui::Context, wanted: ShadeBounds) {
    {
        let Ok(mut guard) = state().lock() else {
            return;
        };
        if guard.pending.is_some() || guard.fetching.is_some() {
            return;
        }
        let covered = guard
            .loaded
            .as_ref()
            .is_some_and(|loaded| loaded.bounds.contains_with_margin(&wanted, 0.0));
        if covered {
            return;
        }
        guard.fetching = Some(wanted);
    }

    let ctx = ctx.clone();
    if std::thread::Builder::new()
        .name("threedep-hillshade".into())
        .spawn(move || {
            let image = crate::threedep::fetch_hillshade_png(
                wanted.min_lat,
                wanted.min_lon,
                wanted.max_lat,
                wanted.max_lon,
                TEXTURE_PX,
                TEXTURE_PX,
            )
            .and_then(|bytes| decode_shade(&bytes));

            if let Ok(mut guard) = state().lock() {
                guard.fetching = None;
                if let Some(image) = image {
                    guard.pending = Some((wanted, image));
                }
            }
            ctx.request_repaint();
        })
        .is_err()
        && let Ok(mut guard) = state().lock()
    {
        guard.fetching = None;
    }
}

/// Decode the service's greyscale PNG into a premultiplied image whose alpha
/// carries the shading. Painting darkness as alpha rather than as opaque grey
/// lets the contour and fill colours below stay visible through the lit slopes.
fn decode_shade(bytes: &[u8]) -> Option<egui::ColorImage> {
    let decoded = image::load_from_memory(bytes).ok()?.to_luma8();
    let (width, height) = decoded.dimensions();
    let pixels = decoded
        .pixels()
        .map(|pixel| {
            let luminance = pixel.0[0];
            // 255 is fully lit and contributes nothing; 0 is deep shadow.
            let shadow = 255u8.saturating_sub(luminance);
            let alpha = (shadow as f32 * 0.55) as u8;
            egui::Color32::from_rgba_premultiplied(0, 0, 0, alpha)
        })
        .collect();
    Some(egui::ColorImage {
        size: [width as usize, height as usize],
        pixels,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds(min_lat: f32, min_lon: f32, max_lat: f32, max_lon: f32) -> ShadeBounds {
        ShadeBounds {
            min_lat,
            min_lon,
            max_lat,
            max_lon,
        }
    }

    #[test]
    fn a_view_inside_the_texture_reuses_it() {
        let cached = bounds(40.0, -105.0, 40.1, -104.9);
        let view = bounds(40.02, -104.98, 40.08, -104.92);
        assert!(cached.contains_with_margin(&view, 0.0));
    }

    #[test]
    fn a_small_pan_stays_within_the_reuse_margin() {
        let cached = bounds(40.0, -105.0, 40.1, -104.9);
        // Slid north by a third of the view height: outside the strict bounds
        // but inside the reuse margin, so the texture is still drawn.
        let view = bounds(40.03, -105.0, 40.13, -104.9);
        assert!(!cached.contains_with_margin(&view, 0.0));
        assert!(cached.contains_with_margin(&view, REUSE_FRACTION));
    }

    #[test]
    fn a_jump_to_another_area_drops_the_texture() {
        let cached = bounds(40.0, -105.0, 40.1, -104.9);
        let view = bounds(37.7, -122.5, 37.8, -122.4);
        assert!(!cached.contains_with_margin(&view, REUSE_FRACTION));
    }

    #[test]
    fn lit_ground_is_transparent_and_shadow_is_dark() {
        let mut lit = image::GrayImage::new(2, 1);
        lit.put_pixel(0, 0, image::Luma([255]));
        lit.put_pixel(1, 0, image::Luma([0]));
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageLuma8(lit)
            .write_to(&mut png, image::ImageFormat::Png)
            .expect("encode test png");

        let decoded = decode_shade(&png.into_inner()).expect("decode");
        assert_eq!(decoded.pixels[0].a(), 0);
        assert!(decoded.pixels[1].a() > 100);
    }
}
