//! Selected public hillshade projected onto the local sea-level plane.
//!
//! The image service supplies relief shading from its own elevation data.
//! This optional texture overlay does not generate terrain geometry or occlude
//! contour lines; its geographic projection matches the local map.

use crate::hillshade_source::{Bounds as ShadeBounds, Source};
use crate::model::GeoPoint;
use crate::model::GlobeViewState;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::projection::project_local;
use super::{LocalLayout, visual_half_extent_for_zoom};

/// Requested texture edge. 1024 is a ~700 KB PNG and resolves better than one
/// screen pixel per texel across the deep zoom range.
const TEXTURE_PX: u32 = 1024;
/// Grid resolution for projecting and clipping the texture onto the map.
const DRAPE_CELLS: usize = 48;
/// Panning within this fraction of the view keeps the current texture instead
/// of refetching, so a slow drag does not stream a new image every frame.
const REUSE_FRACTION: f32 = 0.35;
const RETRY_DELAY: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, PartialEq)]
struct ShadeRequest {
    source: Source,
    bounds: ShadeBounds,
}

impl ShadeRequest {
    fn covers(self, wanted: Self, margin: f32) -> bool {
        self.source == wanted.source && self.bounds.contains_with_margin(&wanted.bounds, margin)
    }
}

struct LoadedShade {
    request: ShadeRequest,
    texture: egui::TextureHandle,
}

#[derive(Default)]
struct ShadeState {
    loaded: Option<LoadedShade>,
    /// Keep one worker even across source changes and cache clears.
    fetching: Option<(u64, ShadeRequest)>,
    generation: u64,
    /// Decoded image waiting to be uploaded on the UI thread.
    pending: Option<(ShadeRequest, egui::ColorImage)>,
    retry_after: Option<(Source, Instant)>,
}

impl ShadeState {
    fn clear(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.loaded = None;
        self.pending = None;
        self.retry_after = None;
        // The old worker still owns the download slot until it returns.
    }

    fn finish(&mut self, generation: u64, request: ShadeRequest, image: Option<egui::ColorImage>) {
        if self.fetching != Some((generation, request)) {
            return;
        }
        self.fetching = None;
        if self.generation != generation {
            return;
        }
        if let Some(image) = image {
            self.pending = Some((request, image));
            self.retry_after = None;
        } else {
            self.retry_after = Some((request.source, Instant::now() + RETRY_DELAY));
        }
    }

    fn cooling_down(&self, source: Source, now: Instant) -> bool {
        self.retry_after
            .is_some_and(|(failed, until)| failed == source && now < until)
    }
}

fn state() -> &'static Mutex<ShadeState> {
    static STATE: OnceLock<Mutex<ShadeState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(ShadeState::default()))
}

/// Drop the cached texture. Wired to the same cache-reset control as the other
/// terrain layers.
pub(crate) fn clear() {
    if let Ok(mut guard) = state().lock() {
        guard.clear();
    }
}

/// Draw the hillshade for the current view, fetching it in the background when
/// the view has moved beyond the cached image.
///
/// The image is projected at sea level, independently of contour elevations.
pub(super) fn draw_hillshade(
    painter: &egui::Painter,
    layout: &LocalLayout,
    view: &GlobeViewState,
    source: Source,
    focus: GeoPoint,
    extent_x_km: f32,
    extent_y_km: f32,
) {
    let half_extent_deg = visual_half_extent_for_zoom(view.local_zoom);
    let wanted = ShadeRequest {
        source,
        bounds: ShadeBounds::around(focus, half_extent_deg),
    };
    if !source.covers(wanted.bounds) {
        return;
    }

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
    if !loaded.request.covers(wanted, REUSE_FRACTION) {
        return;
    }
    let bounds = loaded.request.bounds;
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
            match project_local(layout, view, focus, point, 0.0, extent_x_km, extent_y_km) {
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
    let Some((request, image)) = guard.pending.take() else {
        return;
    };
    let texture = ctx.load_texture(
        "public_hillshade",
        image,
        egui::TextureOptions {
            magnification: egui::TextureFilter::Linear,
            minification: egui::TextureFilter::Linear,
            wrap_mode: egui::TextureWrapMode::ClampToEdge,
            mipmap_mode: None,
        },
    );
    guard.loaded = Some(LoadedShade { request, texture });
}

/// Schedule a fetch when the cached texture no longer covers the view.
fn ensure_requested(ctx: &egui::Context, wanted: ShadeRequest) {
    let generation = {
        let Ok(mut guard) = state().lock() else {
            return;
        };
        if guard.pending.is_some() || guard.fetching.is_some() {
            return;
        }
        if guard.cooling_down(wanted.source, Instant::now()) {
            ctx.request_repaint_after(RETRY_DELAY);
            return;
        }
        let covered = guard
            .loaded
            .as_ref()
            .is_some_and(|loaded| loaded.request.covers(wanted, 0.0));
        if covered {
            return;
        }
        let generation = guard.generation;
        guard.fetching = Some((generation, wanted));
        generation
    };

    let ctx = ctx.clone();
    if std::thread::Builder::new()
        .name("public-hillshade".into())
        .spawn(move || {
            let image = wanted
                .source
                .fetch(wanted.bounds, TEXTURE_PX, TEXTURE_PX)
                .and_then(|bytes| decode_shade(&bytes, wanted.source));

            if let Ok(mut guard) = state().lock() {
                guard.finish(generation, wanted, image);
            }
            ctx.request_repaint();
        })
        .is_err()
        && let Ok(mut guard) = state().lock()
    {
        guard.finish(generation, wanted, None);
    }
}

/// Preserve DNR grayscale relief so it remains visible on a black map. USGS
/// retains its existing shadow-only styling. Both preserve missing-data alpha.
fn decode_shade(bytes: &[u8], source: Source) -> Option<egui::ColorImage> {
    let reader =
        image::ImageReader::with_format(std::io::Cursor::new(bytes), image::ImageFormat::Png);
    let (width, height) = reader.into_dimensions().ok()?;
    if width == 0
        || height == 0
        || width > crate::hillshade_source::MAX_IMAGE_PX
        || height > crate::hillshade_source::MAX_IMAGE_PX
    {
        return None;
    }
    let decoded = image::load_from_memory(bytes).ok()?.to_luma_alpha8();
    let (width, height) = decoded.dimensions();
    let pixels = decoded
        .pixels()
        .map(|pixel| {
            let luminance = pixel.0[0];
            if source != Source::Usgs {
                return egui::Color32::from_rgba_unmultiplied(
                    luminance, luminance, luminance, pixel.0[1],
                );
            }
            // 255 is fully lit and contributes nothing; 0 is deep shadow.
            let shadow = 255u8.saturating_sub(luminance);
            let alpha = (shadow as f32 * 0.55 * f32::from(pixel.0[1]) / 255.0) as u8;
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

        let decoded = decode_shade(&png.into_inner(), Source::Usgs).expect("decode");
        assert_eq!(decoded.pixels[0].a(), 0);
        assert!(decoded.pixels[1].a() > 100);
    }

    #[test]
    fn transparent_nodata_never_becomes_shadow() {
        let mut rgba = image::RgbaImage::new(3, 1);
        rgba.put_pixel(0, 0, image::Rgba([0, 0, 0, 0]));
        rgba.put_pixel(1, 0, image::Rgba([0, 0, 0, 128]));
        rgba.put_pixel(2, 0, image::Rgba([0, 0, 0, 255]));
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(rgba)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let bytes = png.into_inner();
        let decoded = decode_shade(&bytes, Source::Usgs).unwrap();
        assert_eq!(decoded.pixels[0].a(), 0);
        assert!((69..=71).contains(&decoded.pixels[1].a()));
        assert_eq!(decoded.pixels[2].a(), 140);
        let dnr = decode_shade(&bytes, Source::WashingtonBareEarth).unwrap();
        assert_eq!(dnr.pixels[0], egui::Color32::TRANSPARENT);
        assert_eq!(dnr.pixels[1].a(), 128);
        assert_eq!(dnr.pixels[2].a(), 255);
        assert!(decode_shade(br#"{"error":{"code":500}}"#, Source::WashingtonSurface).is_none());
    }

    #[test]
    fn excessive_png_dimensions_are_rejected() {
        let image = image::GrayImage::new(crate::hillshade_source::MAX_IMAGE_PX + 1, 1);
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageLuma8(image)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        assert!(decode_shade(&png.into_inner(), Source::WashingtonBareEarth).is_none());
    }

    #[test]
    fn washington_relief_is_visible_on_a_black_canvas() {
        let relief = image::GrayImage::from_raw(2, 1, vec![64, 220]).unwrap();
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageLuma8(relief)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        for source in [Source::WashingtonBareEarth, Source::WashingtonSurface] {
            let decoded = decode_shade(png.get_ref(), source).unwrap();
            assert_eq!(decoded.pixels[0], egui::Color32::from_gray(64));
            assert_eq!(decoded.pixels[1], egui::Color32::from_gray(220));
        }
    }

    #[test]
    fn source_changes_do_not_reuse_another_products_texture() {
        let request = ShadeRequest {
            source: Source::WashingtonBareEarth,
            bounds: bounds(47.59, -122.36, 47.63, -122.32),
        };
        assert!(request.covers(request, 0.0));
        for source in [Source::Usgs, Source::WashingtonSurface] {
            assert!(!request.covers(ShadeRequest { source, ..request }, REUSE_FRACTION));
        }
    }

    #[test]
    fn cleared_cache_discards_inflight_results_and_failures_back_off() {
        let request = ShadeRequest {
            source: Source::WashingtonBareEarth,
            bounds: bounds(47.59, -122.36, 47.63, -122.32),
        };
        let mut state = ShadeState {
            fetching: Some((0, request)),
            ..Default::default()
        };
        state.clear();
        assert!(
            state.fetching.is_some(),
            "cache reset must keep the one-worker slot"
        );
        state.finish(
            0,
            request,
            Some(egui::ColorImage::new([1, 1], egui::Color32::BLACK)),
        );
        assert!(state.fetching.is_none());
        assert!(state.pending.is_none());
        state.fetching = Some((1, request));
        state.finish(1, request, None);
        let now = Instant::now();
        assert!(state.cooling_down(request.source, now));
        assert!(!state.cooling_down(Source::WashingtonSurface, now));
        assert!(!state.cooling_down(request.source, now + RETRY_DELAY));
    }
}
