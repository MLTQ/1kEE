//! Cached Atlas base map shared by Globe and Local views.
use super::{atlas_image::HeightGrid, globe_scene::GlobeLayout, local_terrain_scene::LocalLayout};
use crate::{
    model::{GeoPoint, GlobeViewState},
    terrain_assets,
};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

type Images = super::layer_snapshot::Snapshot<Option<PathBuf>, egui::TextureHandle>;
static IMAGE: OnceLock<Mutex<Images>> = OnceLock::new();
type MeshCache = Option<(Vec<u32>, egui::TextureId, Arc<egui::Mesh>)>;
static MESH: OnceLock<Mutex<[MeshCache; 4]>> = OnceLock::new();

#[derive(Clone, PartialEq, Eq)]
struct DetailRequest {
    root: Option<PathBuf>,
    region: super::atlas_viewport::Region,
    max_edge: usize,
    epoch: u64,
}
#[derive(Clone)]
struct DetailNotice {
    request: DetailRequest,
    message: Option<String>,
}

struct LoadedDetail {
    texture: egui::TextureHandle,
    region: super::atlas_viewport::Region,
}
type Details = super::layer_snapshot::Snapshot<DetailRequest, LoadedDetail>;
static GLOBE_DETAIL: OnceLock<Mutex<Details>> = OnceLock::new();
static LOCAL_DETAIL: OnceLock<Mutex<Details>> = OnceLock::new();

fn detail_texture(
    ctx: &egui::Context,
    root: Option<&Path>,
    region: super::atlas_viewport::Region,
    local: bool,
) -> Option<Arc<LoadedDetail>> {
    let wake = ctx.clone();
    let state = if local { &LOCAL_DETAIL } else { &GLOBE_DETAIL };
    let mut cache = state.get_or_init(Default::default).lock().ok()?;
    let request = DetailRequest {
        root: root.map(Path::to_path_buf),
        region,
        max_edge: ctx.input(|i| i.max_texture_side).clamp(16, 8192),
        epoch: cache.epoch(),
    };
    ctx.data_mut(|d| d.insert_temp(egui::Id::new(("atlas-wanted", local)), request.clone()));
    let result = cache.get(
        request,
        "atlas-native-detail",
        |a, b| a.root == b.root,
        move |request| {
            let result = load_detail(&wake, &request);
            let message = result.as_ref().err().cloned();
            wake.data_mut(|d| {
                d.insert_temp(
                    egui::Id::new(("atlas-notice", local)),
                    DetailNotice { request, message },
                )
            });
            wake.request_repaint();
            result.ok()
        },
    );
    if result.is_none() {
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
    }
    result
}

fn load_detail(ctx: &egui::Context, request: &DetailRequest) -> Result<LoadedDetail, String> {
    let detail =
        super::atlas_source::load(request.root.as_deref(), request.region, request.max_edge)?;
    let image = detail
        .grid
        .image_for(detail.region.bounds())
        .ok_or("Atlas terrain cannot be shaded: invalid image bounds.")?;
    Ok(LoadedDetail {
        texture: ctx.load_texture(detail.source, image, egui::TextureOptions::LINEAR),
        region: detail.region,
    })
}

fn paint_detail_notice(painter: &egui::Painter, rect: egui::Rect, local: bool) {
    let message = painter.ctx().data(|d| {
        let wanted = d.get_temp::<DetailRequest>(egui::Id::new(("atlas-wanted", local)))?;
        match d.get_temp::<DetailNotice>(egui::Id::new(("atlas-notice", local))) {
            Some(notice) if notice.request == wanted => notice
                .message
                .map(|m| format!("Atlas detail unavailable: {m}")),
            _ => Some("Atlas: refining terrain…".into()),
        }
    });
    let Some(message) = message else {
        return;
    };
    let width = (rect.width() - 40.0).clamp(80.0, 440.0);
    let galley = painter.layout(
        message,
        egui::FontId::proportional(11.0),
        crate::theme::text_muted(),
        width,
    );
    let pos = rect.left_top() + egui::vec2(12.0, 12.0);
    let panel = egui::Rect::from_min_size(pos, galley.size() + egui::vec2(16.0, 12.0));
    painter.rect_filled(panel, 3.0, crate::theme::panel_fill(240));
    painter.galley(
        pos + egui::vec2(8.0, 6.0),
        galley,
        crate::theme::text_muted(),
    );
}

const WORLD: [f32; 4] = [-180.0, -90.0, 180.0, 90.0];

fn texture(ctx: &egui::Context, root: Option<&Path>) -> Option<egui::TextureId> {
    let wake = ctx.clone();
    let result = IMAGE.get_or_init(Default::default).lock().ok()?.get(
        root.map(Path::to_path_buf),
        "atlas-relief",
        |a, b| a == b,
        move |root| {
            let image = load_grid(root.as_deref())?.image()?;
            Some(wake.load_texture("atlas-relief", image, egui::TextureOptions::LINEAR))
        },
    );
    if result.is_none() {
        ctx.request_repaint_after(std::time::Duration::from_secs(1));
    }
    result.map(|image| image.id())
}

/// All discovery, file reads, decoding and lighting run on the snapshot worker.
fn load_grid(root: Option<&Path>) -> Option<HeightGrid> {
    let directory = terrain_assets::find_derived_root(root)?.join("terrain");
    for (name, signed) in [
        ("gebco_2025_preview_4096.tif", true),
        ("gebco_2025_preview_4096.png", false),
    ] {
        if let Some(grid) = read_preview(&directory.join(name), signed) {
            return Some(grid);
        }
    }
    let bytes = std::fs::read(directory.join("gebco_depth_1440x720.bil")).ok()?;
    if bytes.len() != 1440 * 720 * 2 {
        return None;
    }
    Some(HeightGrid {
        width: 1440,
        height: 720,
        metres: bytes
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32)
            .collect(),
    })
}

fn read_preview(path: &Path, signed: bool) -> Option<HeightGrid> {
    let mut reader = image::ImageReader::open(path).ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(2048);
    reader.limits(limits);
    let image::DynamicImage::ImageLuma16(image) = reader.decode().ok()? else {
        return None;
    };
    let (width, height) = image.dimensions();
    if width != height * 2 {
        return None;
    }
    let metres = image
        .into_raw()
        .into_iter()
        .map(|value| {
            if signed {
                (value as i16) as f32
            } else {
                -11_000.0 + value as f32 / 65535.0 * 20_000.0
            }
        })
        .collect();
    Some(HeightGrid {
        width: width as usize,
        height: height as usize,
        metres,
    })
}

fn paint_cached(
    painter: &egui::Painter,
    texture: egui::TextureId,
    index: usize,
    key: Vec<u32>,
    build: impl FnOnce() -> egui::Mesh,
) {
    let Ok(mut caches) = MESH.get_or_init(Default::default).lock() else {
        return;
    };
    let cache = &mut caches[index];
    if cache
        .as_ref()
        .is_none_or(|(old, id, _)| old != &key || *id != texture)
    {
        *cache = Some((key, texture, Arc::new(build())));
    }
    if let Some((_, _, mesh)) = cache.as_ref() {
        painter.add(egui::Shape::mesh(mesh.clone()));
    }
}

pub(super) fn draw_globe(
    painter: &egui::Painter,
    rect: egui::Rect,
    layout: &GlobeLayout,
    view: &GlobeViewState,
    root: Option<&Path>,
) {
    let key = vec![
        0,
        view.yaw.to_bits(),
        view.pitch.to_bits(),
        layout.center.x.to_bits(),
        layout.center.y.to_bits(),
        layout.radius.to_bits(),
        layout.focal_length.to_bits(),
        layout.camera_distance.to_bits(),
    ];
    if let Some(texture) = texture(painter.ctx(), root) {
        paint_cached(painter, texture, 0, key.clone(), || {
            super::raster_mesh::globe(layout, view, texture, WORLD, false)
        });
    }
    if let Some(region) =
        super::atlas_viewport::globe(rect, layout, view, painter.ctx().pixels_per_point())
    {
        if let Some(detail) = detail_texture(painter.ctx(), root, region, false) {
            let texture = detail.texture.id();
            paint_cached(painter, texture, 1, key, || {
                super::raster_mesh::globe(layout, view, texture, detail.region.bounds(), true)
            });
        }
        paint_detail_notice(painter, rect, false);
    }
}

pub(super) fn draw_local(
    painter: &egui::Painter,
    layout: &LocalLayout,
    view: &GlobeViewState,
    root: Option<&Path>,
    focus: GeoPoint,
    half: f32,
) {
    let clip = painter.clip_rect();
    let key = vec![
        1,
        view.local_yaw.to_bits(),
        view.local_pitch.to_bits(),
        view.local_layer_spread.to_bits(),
        focus.lat.to_bits(),
        focus.lon.to_bits(),
        half.to_bits(),
        layout.focus_center.x.to_bits(),
        layout.focus_center.y.to_bits(),
        layout.width.to_bits(),
        layout.height.to_bits(),
        layout.horizontal_scale.to_bits(),
        clip.min.x.to_bits(),
        clip.min.y.to_bits(),
        clip.max.x.to_bits(),
        clip.max.y.to_bits(),
    ];
    if let Some(texture) = texture(painter.ctx(), root) {
        paint_cached(painter, texture, 2, key.clone(), || {
            local_mesh(layout, view, texture, focus, half, clip)
        });
    }
    if let Some(region) = super::atlas_viewport::local(clip, layout, view, focus, half) {
        if let Some(detail) = detail_texture(painter.ctx(), root, region, true) {
            let texture = detail.texture.id();
            paint_cached(painter, texture, 3, key, || {
                super::raster_mesh::local(
                    layout,
                    view,
                    texture,
                    detail.region.bounds(),
                    focus,
                    half,
                    clip,
                )
            });
        }
        paint_detail_notice(painter, clip, true);
    }
}

fn local_mesh(
    layout: &LocalLayout,
    view: &GlobeViewState,
    texture: egui::TextureId,
    focus: GeoPoint,
    half: f32,
    clip: egui::Rect,
) -> egui::Mesh {
    let mut mesh = egui::Mesh::with_texture(texture);
    // Separate longitude quarters unwrap independently around the focus,
    // so Local remains continuous when a viewport crosses the dateline.
    for west in [-180.0, -90.0, 0.0, 90.0] {
        let mut part = super::raster_mesh::local(
            layout,
            view,
            texture,
            [west, -90.0, west + 90.0, 90.0],
            focus,
            half,
            clip,
        );
        for vertex in &mut part.vertices {
            vertex.uv.x = (west + 180.0) / 360.0 + vertex.uv.x * 0.25;
        }
        mesh.append(part);
    }
    mesh
}

pub(super) fn clear() {
    if let Some(cache) = IMAGE.get() {
        cache.lock().unwrap().clear();
    }
    for cache in [&GLOBE_DETAIL, &LOCAL_DETAIL] {
        if let Some(cache) = cache.get() {
            cache.lock().unwrap().clear();
        }
    }
    if let Some(cache) = MESH.get() {
        *cache.lock().unwrap() = Default::default();
    }
}

#[cfg(test)]
#[path = "atlas_relief_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "atlas_runtime_tests.rs"]
mod runtime_tests;
