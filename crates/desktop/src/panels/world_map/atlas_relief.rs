//! Cached Atlas base map shared by Globe and Local views.
use super::{atlas_image::HeightGrid, globe_scene::GlobeLayout, local_terrain_scene::LocalLayout};
#[cfg(test)]
use crate::model::GlobeViewState;
use crate::{model::GeoPoint, terrain_assets};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

type Images = super::layer_snapshot::Snapshot<Option<PathBuf>, Arc<super::atlas_surface::Raster>>;
static IMAGE: OnceLock<Mutex<Images>> = OnceLock::new();

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

#[derive(Clone)]
struct LoadedDetail {
    raster: Arc<super::atlas_surface::Raster>,
}
type Details = super::layer_snapshot::Snapshot<DetailRequest, LoadedDetail>;
static GLOBE_DETAIL: OnceLock<Mutex<Details>> = OnceLock::new();
static LOCAL_DETAIL: OnceLock<Mutex<Details>> = OnceLock::new();

#[cfg(test)]
fn detail_texture(
    ctx: &egui::Context,
    root: Option<&Path>,
    region: super::atlas_viewport::Region,
    local: bool,
) -> Option<Arc<LoadedDetail>> {
    detail_texture_visible(ctx, root, region, region.bounds(), local)
}

impl DetailRequest {
    fn reusable_for(&self, wanted: &Self, visible: [f32; 4]) -> bool {
        let old = self.region.span();
        let new = wanted.region.span();
        let tier = |span: [f32; 2]| {
            if span[0] <= 0.15 && span[1] <= 0.15 {
                2
            } else if span[0] <= 8. && span[1] <= 8. {
                1
            } else {
                0
            }
        };
        self.root == wanted.root
            && self.epoch == wanted.epoch
            && self.max_edge == wanted.max_edge
            && old.iter().zip(new).all(|(old, new)| *old <= new * 1.25)
            && tier(old) == tier(new)
            && self.region.contains(visible)
    }
}

fn detail_texture_visible(
    ctx: &egui::Context,
    root: Option<&Path>,
    region: super::atlas_viewport::Region,
    visible: [f32; 4],
    local: bool,
) -> Option<Arc<LoadedDetail>> {
    let wake = ctx.clone();
    let state = if local { &LOCAL_DETAIL } else { &GLOBE_DETAIL };
    let mut cache = state.get_or_init(Default::default).lock().ok()?;
    let mut request = DetailRequest {
        root: root.map(Path::to_path_buf),
        region,
        max_edge: ctx.input(|i| i.max_texture_side).clamp(16, 8192),
        epoch: cache.epoch(),
    };
    if let Some(previous) =
        ctx.data(|d| d.get_temp::<DetailRequest>(egui::Id::new(("atlas-wanted", local))))
        && previous.reusable_for(&request, visible)
    {
        request.region = previous.region;
    }
    // A warm view must not queue behind an unrelated, potentially slow GDAL
    // read. Search retained coverage as well as the exact padded window.
    if let Some((key, detail)) = detail_cache(local).lock().ok()?.covering(&request, visible) {
        request = key;
        cache.install(request.clone(), detail);
        ctx.data_mut(|d| {
            d.insert_temp(
                egui::Id::new(("atlas-notice", local)),
                DetailNotice {
                    request: request.clone(),
                    message: None,
                },
            );
        });
    }
    ctx.data_mut(|d| d.insert_temp(egui::Id::new(("atlas-wanted", local)), request.clone()));
    let recent = detail_cache(local);
    let generation = recent.lock().ok()?.generation;
    let result = cache.get(
        request,
        "atlas-native-detail",
        |a, b| a.root == b.root,
        move |request| {
            let result = load_detail_from(&request, recent, generation);
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

struct RecentDetails {
    entries: std::collections::VecDeque<(DetailRequest, LoadedDetail)>,
    bytes: usize,
    budget: usize,
    resident: bool,
    generation: u64,
}
impl Default for RecentDetails {
    fn default() -> Self {
        Self {
            entries: Default::default(),
            bytes: 0,
            budget: crate::gpu_config::settings().atlas_cpu_bytes,
            resident: false,
            generation: 0,
        }
    }
}
impl RecentDetails {
    fn resident() -> Self {
        Self {
            resident: true,
            ..Default::default()
        }
    }
    fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
        self.generation = self.generation.wrapping_add(1);
    }
    fn publish(&mut self, generation: u64, request: DetailRequest, detail: LoadedDetail) {
        if self.generation == generation {
            self.insert(request, detail);
        }
    }
    fn covering(
        &mut self,
        request: &DetailRequest,
        visible: [f32; 4],
    ) -> Option<(DetailRequest, LoadedDetail)> {
        let index = self
            .entries
            .iter()
            .rposition(|(key, _)| key == request || key.reusable_for(request, visible))?;
        let entry = self.entries.remove(index)?;
        let result = entry.clone();
        self.entries.push_back(entry);
        Some(result)
    }
    fn get(&mut self, request: &DetailRequest) -> Option<LoadedDetail> {
        let index = self.entries.iter().position(|(key, _)| key == request)?;
        let entry = self.entries.remove(index)?;
        let result = entry.1.clone();
        self.entries.push_back(entry);
        Some(result)
    }
    fn insert(&mut self, request: DetailRequest, detail: LoadedDetail) {
        let bytes = detail.raster.heights.len() * 4 + detail.raster.colors.len();
        if !self.resident && bytes > self.budget {
            return;
        }
        if self.get(&request).is_some() {
            return;
        }
        while !self.resident && self.bytes + bytes > self.budget {
            if let Some((_, old)) = self.entries.pop_front() {
                self.bytes -= old.raster.heights.len() * 4 + old.raster.colors.len();
            } else {
                break;
            }
        }
        self.bytes += bytes;
        self.entries.push_back((request, detail));
        if self.resident {
            eprintln!(
                "[atlas] Globe retained: {} regions, {:.1} MiB prepared RAM",
                self.entries.len(),
                self.bytes as f64 / 1048576.
            );
        }
    }
}
static RECENT: OnceLock<Mutex<RecentDetails>> = OnceLock::new();
static GLOBE_RESIDENT: OnceLock<Mutex<RecentDetails>> = OnceLock::new();

fn detail_cache(local: bool) -> &'static Mutex<RecentDetails> {
    if !local && crate::gpu_config::settings().atlas_keep_globe {
        GLOBE_RESIDENT.get_or_init(|| Mutex::new(RecentDetails::resident()))
    } else {
        RECENT.get_or_init(Default::default)
    }
}

#[cfg(test)]
fn load_detail(request: &DetailRequest) -> Result<LoadedDetail, String> {
    let recent = RECENT.get_or_init(Default::default);
    let generation = recent.lock().map_err(|e| e.to_string())?.generation;
    load_detail_from(request, recent, generation)
}

fn load_detail_from(
    request: &DetailRequest,
    recent: &Mutex<RecentDetails>,
    generation: u64,
) -> Result<LoadedDetail, String> {
    {
        let mut cache = recent.lock().map_err(|e| e.to_string())?;
        if cache.generation != generation {
            return Err("Atlas request was invalidated".into());
        }
        if let Some(detail) = cache.get(request) {
            return Ok(detail);
        }
    }
    let detail =
        super::atlas_source::load(request.root.as_deref(), request.region, request.max_edge)?;
    let complete = detail.complete;
    let raster = detail
        .into_raster()
        .ok_or("Atlas terrain cannot be shaded: invalid image bounds.")?;
    let detail = LoadedDetail { raster };
    if complete {
        let mut cache = recent.lock().map_err(|e| e.to_string())?;
        // A worker that was running during Cache Blast must not re-pin data.
        cache.publish(generation, request.clone(), detail.clone());
    }
    Ok(detail)
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
    let message = message.or_else(|| {
        let slot = usize::from(local);
        let wanted = painter
            .ctx()
            .data(|d| d.get_temp::<u64>(egui::Id::new(("atlas-surface-wanted", slot))))?;
        (!super::atlas_surface::uploaded(painter.ctx(), slot, wanted))
            .then(|| "Atlas: preparing 3D terrain…".into())
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

fn texture(ctx: &egui::Context, root: Option<&Path>) -> Option<Arc<super::atlas_surface::Raster>> {
    let result = IMAGE.get_or_init(Default::default).lock().ok()?.get(
        root.map(Path::to_path_buf),
        "atlas-relief",
        |a, b| a == b,
        move |root| super::atlas_surface::Raster::new(load_grid(root.as_deref())?, WORLD),
    );
    if result.is_none() {
        ctx.request_repaint_after(std::time::Duration::from_secs(1));
    }
    result.map(|image| image.as_ref().clone())
}

/// All discovery, file reads, decoding and lighting run on the snapshot worker.
fn load_grid(root: Option<&Path>) -> Option<HeightGrid> {
    let directory = terrain_assets::find_derived_root(root)?.join("terrain");
    if let Ok(mut cache) =
        tile_archive::atlas::Reader::open(&directory.join(tile_archive::atlas::FILE_NAME))
        && let Ok(Some(metres)) = cache.sample(
            [-180., -90., 180., 90.],
            [4096, 2048],
            tile_archive::atlas::Tier::Overview,
        )
    {
        return Some(HeightGrid {
            width: 4096,
            height: 2048,
            metres,
        });
    }
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

pub(super) fn draw_globe(
    painter: &egui::Painter,
    rect: egui::Rect,
    layout: &GlobeLayout,
    model: &crate::model::AppModel,
) {
    let view = &model.globe_view;
    let root = model.selected_root.as_deref();
    let mut rasters = Vec::new();
    if let Some(overview) = texture(painter.ctx(), root) {
        rasters.push(overview);
    }
    let visible =
        super::atlas_viewport::globe_visible(rect, layout, view, painter.ctx().pixels_per_point());
    let region = visible.and_then(super::atlas_viewport::Region::padded);
    if let (Some(region), Some(visible)) = (region, visible)
        && let Some(detail) = detail_texture_visible(painter.ctx(), root, region, visible, false)
    {
        rasters.push(detail.raster.clone());
    }
    if !rasters.is_empty() {
        super::atlas_surface::Callback::globe(
            painter,
            rect,
            layout,
            view,
            rasters,
            model.show_contours,
            model.show_bathymetry,
            model.contour_stroke_width_px(painter.ctx().pixels_per_point()),
        )
        .with_style(model.atlas_style, model.stellar_jd)
        .with_coastlines(model.show_coastlines)
        .submit(painter);
    }
    if region.is_some() {
        paint_detail_notice(painter, rect, false);
    }
}

pub(super) fn draw_local(
    painter: &egui::Painter,
    layout: &LocalLayout,
    model: &crate::model::AppModel,
    focus: GeoPoint,
    half: f32,
) {
    if !model.show_trees {
        super::atlas_forest::suspend();
    }
    let view = &model.globe_view;
    let root = model.selected_root.as_deref();
    if let Some(visible) =
        super::atlas_viewport::local_visible(painter.clip_rect(), layout, view, focus, half)
        && let Some(region) = super::atlas_viewport::Region::padded(visible)
    {
        if let Some(detail) = detail_texture_visible(painter.ctx(), root, region, visible, true) {
            super::atlas_surface::Callback::local(
                painter,
                layout,
                view,
                detail.raster.clone(),
                focus,
                half,
                model.show_contours,
                model.show_bathymetry,
                model.contour_stroke_width_px(painter.ctx().pixels_per_point()),
            )
            .with_forest(if model.show_trees {
                super::atlas_forest::get(painter.ctx(), root, &detail.raster)
            } else {
                None
            })
            .with_style(model.atlas_style, model.stellar_jd)
            .with_coastlines(model.show_coastlines)
            .submit(painter);
        }
        paint_detail_notice(painter, painter.clip_rect(), true);
    }
}

pub(super) fn clear() {
    super::atlas_surface::clear();
    super::atlas_forest::clear();
    for recent in [&RECENT, &GLOBE_RESIDENT] {
        if let Some(recent) = recent.get() {
            recent.lock().unwrap().clear();
        }
    }
    if let Some(cache) = IMAGE.get() {
        cache.lock().unwrap().clear();
    }
    for cache in [&GLOBE_DETAIL, &LOCAL_DETAIL] {
        if let Some(cache) = cache.get() {
            cache.lock().unwrap().clear();
        }
    }
}

#[cfg(test)]
#[path = "atlas_relief_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "atlas_runtime_tests.rs"]
mod runtime_tests;

#[cfg(test)]
#[path = "atlas_load_bench.rs"]
mod load_bench;
