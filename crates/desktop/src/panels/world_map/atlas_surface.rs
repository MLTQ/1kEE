//! Elevation-displaced Atlas geometry. Contours are isolines on that same surface.
use super::{
    atlas_image::HeightGrid,
    globe_scene::GlobeLayout,
    local_terrain_scene::{LocalLayout, projection::local_projection_params},
};
use crate::model::{GeoPoint, GlobeViewState};
use eframe::{egui_wgpu, wgpu};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

#[path = "atlas_surface_gpu.rs"]
mod gpu;

pub(super) struct Raster {
    pub id: u64,
    pub size: [usize; 2],
    pub bounds: [f32; 4],
    /// Geographic core of a Globe tile; heights include a shared one-post halo.
    pub core: Option<[f32; 4]>,
    pub heights: Vec<f32>,
    pub colors: Vec<u8>,
}
impl Raster {
    pub fn new(grid: HeightGrid, bounds: [f32; 4]) -> Option<Arc<Self>> {
        let image = grid.image_for(bounds)?;
        Self::prepared(
            grid,
            bounds,
            image.pixels.iter().flat_map(|c| c.to_array()).collect(),
        )
    }
    pub fn prepared(grid: HeightGrid, bounds: [f32; 4], colors: Vec<u8>) -> Option<Arc<Self>> {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let count = grid.width.checked_mul(grid.height)?;
        if grid.width < 3
            || grid.height < 3
            || grid.metres.len() != count
            || colors.len() != count.checked_mul(4)?
            || !bounds.iter().all(|v| v.is_finite())
            || bounds[0] >= bounds[2]
            || bounds[1] >= bounds[3]
        {
            return None;
        }
        Some(Arc::new(Self {
            id: NEXT.fetch_add(1, Ordering::Relaxed),
            size: [grid.width, grid.height],
            bounds,
            core: None,
            heights: grid.metres,
            colors,
        }))
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct Uniforms {
    viewport: [f32; 4],
    region: [f32; 4],
    a: [f32; 4],
    b: [f32; 4],
    c: [f32; 4],
    d: [f32; 4],
    globe: [f32; 4],
    sphere: [f32; 4],
    style: [f32; 4],
    grid: [f32; 4],
    extra: [f32; 4],
    major: [f32; 4],
    minor: [f32; 4],
    exclude: [f32; 4],
    light: [f32; 4],
    appearance: [f32; 4],
}
const _: () = assert!(std::mem::size_of::<Uniforms>() == 256);
impl Uniforms {
    fn reframe(mut self, r: &Raster) -> Self {
        self.region = r.bounds;
        let limit = if r.bounds[2] - r.bounds[0] > 180.0 {
            512
        } else {
            4096
        };
        self.grid[0] = (r.size[0] - 1).min(limit) as f32;
        self.grid[1] = (r.size[1] - 1).min(limit) as f32;
        self.extra[2] = 0.0;
        self.extra[3] = 0.0;
        if self.extra[0] < 0.5
            && let Some(core) = r.core
        {
            self.extra[3] = 1.0;
            self.exclude = core;
            // Bound geometry by screen density, while retaining native height/color textures.
            let pixels_per_degree =
                self.globe[2] / (self.globe[3] - 1.0) * std::f32::consts::PI / 180.0;
            for (axis, span) in [(0, core[2] - core[0]), (1, core[3] - core[1])] {
                self.grid[axis] = self.grid[axis].min((span * pixels_per_degree).ceil().max(16.0));
            }
        }
        if self.extra[0] > 0.5 {
            self.extra[1] =
                ((self.a[0] - (r.bounds[0] + r.bounds[2]) * 0.5) / 360.0).round() * 360.0;
            let mut lo = f32::INFINITY;
            let mut hi = f32::NEG_INFINITY;
            for lon in [r.bounds[0] + self.extra[1], r.bounds[2] + self.extra[1]] {
                for lat in [r.bounds[1], r.bounds[3]] {
                    let g = (lon - self.a[0]) * self.a[2] * self.b[2]
                        + (lat - self.a[1]) * self.a[3] * self.b[1];
                    lo = lo.min(g);
                    hi = hi.max(g);
                }
            }
            if self.globe[0] != 0.0 {
                // Full latitude rings can attain their extremum between corners.
                let pole = self.globe[0];
                let radius = (90.0 - pole * r.bounds[1]).max(90.0 - pole * r.bounds[3]);
                let center = (pole * 90.0 - self.a[1]) * self.a[3] * self.b[1];
                lo = center - radius * self.a[3];
                hi = center + radius * self.a[3];
            }
            self.grid[2] = (hi + lo) * 0.5;
            self.grid[3] = 0.9 / (hi - lo).max(1.0);
        }
        self
    }
}

pub(super) struct Callback {
    pub data: Vec<Arc<Raster>>,
    pub uniforms: Vec<Uniforms>,
    pub rect: egui::Rect,
    pub ctx: egui::Context,
    pub slot: usize,
    pub forest: Option<Arc<super::atlas_forest::Mask>>,
}
impl Callback {
    fn base(
        r: &Raster,
        rect: egui::Rect,
        contours: bool,
        bathymetry: bool,
        interval: f32,
        width: f32,
    ) -> Uniforms {
        let mut u: Uniforms = bytemuck::Zeroable::zeroed();
        u.viewport = [rect.min.x, rect.min.y, rect.width(), rect.height()];
        u.region = r.bounds;
        let limit = if r.bounds[2] - r.bounds[0] > 180.0 {
            512
        } else {
            4096
        };
        u.grid[0] = (r.size[0] - 1).min(limit) as f32;
        u.grid[1] = (r.size[1] - 1).min(limit) as f32;
        u.style = [
            interval.max(1.0),
            width,
            u32::from(contours) as f32,
            u32::from(bathymetry) as f32,
        ];
        u.light = crate::atlas_style::AtlasStyle::default().light(0.0);
        u.appearance = [1.0, 1.0, 0.0, 0.0];
        u.major = super::gpu_color::rgba(crate::theme::hot_color());
        u.minor = super::gpu_color::rgba(crate::theme::contour_color());
        u
    }
    pub fn local(
        painter: &egui::Painter,
        layout: &LocalLayout,
        view: &GlobeViewState,
        data: Arc<Raster>,
        focus: GeoPoint,
        half: f32,
        contours: bool,
        bathymetry: bool,
        width: f32,
    ) -> Self {
        let rect = painter.clip_rect();
        let ex = (half * 111.32 * focus.lat.to_radians().cos().abs().max(0.2)).max(1.0);
        let p = local_projection_params(layout, view, focus, ex, (half * 111.32).max(1.0));
        let interval = super::srtm_focus_cache::contour_interval_for_zoom(view.local_zoom);
        let mut u = Self::base(&data, rect, contours, bathymetry, interval, width);
        u.a = [p.focus_lon, p.focus_lat, p.x_factor, p.y_factor];
        u.b = [p.z_factor, p.yaw_cos, p.yaw_sin, p.pitch_cos];
        u.c = [
            p.pitch_sin,
            p.focus_center_x,
            p.focus_center_y,
            p.horizontal_scale,
        ];
        u.d = [
            p.ground_pitch_scale,
            p.ground_depth_scale,
            p.elevation_pitch_scale,
            p.elevation_depth_scale,
        ];
        u.extra[0] = 1.0;
        u.globe[0] = p.pole;
        u = u.reframe(&data);
        Self {
            data: vec![data],
            uniforms: vec![u],
            rect,
            ctx: painter.ctx().clone(),
            slot: 1,
            forest: None,
        }
    }
    pub fn globe(
        painter: &egui::Painter,
        rect: egui::Rect,
        layout: &GlobeLayout,
        view: &GlobeViewState,
        data: Vec<Arc<Raster>>,
        contours: bool,
        bathymetry: bool,
        width: f32,
    ) -> Self {
        let uniforms = data
            .iter()
            .map(|r| {
                let mut u = Self::base(
                    r,
                    rect,
                    contours,
                    bathymetry,
                    super::srtm_focus_cache::contour_interval_for_zoom(view.zoom).max(50.0),
                    width,
                );
                u.globe = [
                    view.yaw,
                    view.pitch,
                    layout.radius * layout.focal_length,
                    layout.camera_distance,
                ];
                u.sphere = [layout.center.x, layout.center.y, 1.0 / 6_371_000.0, 0.0];
                u
            })
            .collect();
        Self {
            data,
            uniforms,
            rect,
            ctx: painter.ctx().clone(),
            slot: 0,
            forest: None,
        }
    }
    pub fn with_forest(mut self, mask: Option<Arc<super::atlas_forest::Mask>>) -> Self {
        self.forest = mask.filter(|mask| {
            self.data.iter().any(|raster| {
                raster.id == mask.raster_id
                    && raster.size == mask.size
                    && mask.pixels.len() == mask.size[0] * mask.size[1]
            })
        });
        self
    }
    pub fn with_style(mut self, style: crate::atlas_style::AtlasStyle, jd: f64) -> Self {
        let style = style.normalized();
        let light = style.light(jd);
        for u in &mut self.uniforms {
            u.light = light;
            u.appearance = [style.color_richness, style.relief_strength, 0.0, 0.0];
        }
        if style.light_mode == crate::atlas_style::LightMode::Sun {
            self.ctx
                .request_repaint_after(std::time::Duration::from_secs(30));
        }
        self
    }
    pub fn with_coastlines(mut self, enabled: bool) -> Self {
        for u in &mut self.uniforms {
            u.sphere[3] = u32::from(enabled) as f32;
        }
        self
    }
    pub fn submit(self, painter: &egui::Painter) {
        let rect = self.rect;
        self.ctx.data_mut(|d| {
            let id = egui::Id::new("atlas-forest-wanted");
            if let Some(mask) = &self.forest {
                d.insert_temp(id, mask.id);
            } else {
                d.remove::<u64>(id);
            }
        });
        if let Some(last) = self.data.last() {
            painter.ctx().data_mut(|d| {
                d.insert_temp(egui::Id::new(("atlas-surface-wanted", self.slot)), last.id)
            });
        }
        painter.add(egui_wgpu::Callback::new_paint_callback(rect, self));
    }
}
pub(super) fn shader_source() -> String {
    format!(
        "{}\n{}\n{}",
        include_str!("local_ground.wgsl"),
        include_str!("atlas_surface.wgsl"),
        include_str!("atlas_lighting.wgsl")
    )
}
pub(super) fn uploaded(ctx: &egui::Context, slot: usize, id: u64) -> bool {
    ctx.data(|d| d.get_temp::<u64>(egui::Id::new(("atlas-surface-active", slot)))) == Some(id)
}
static GENERATION: AtomicU64 = AtomicU64::new(0);
pub(super) fn clear() {
    GENERATION.fetch_add(1, Ordering::Relaxed);
}
struct Visibility;
impl egui_wgpu::CallbackTrait for Visibility {
    fn prepare(
        &self,
        _: &wgpu::Device,
        _: &wgpu::Queue,
        _: &egui_wgpu::ScreenDescriptor,
        _: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if let Some(res) = resources.get_mut::<gpu::Resources>() {
            res.sync_generation(GENERATION.load(Ordering::Relaxed));
        }
        vec![]
    }
    fn paint(
        &self,
        _: egui::PaintCallbackInfo,
        _: &mut wgpu::RenderPass<'static>,
        _: &egui_wgpu::CallbackResources,
    ) {
    }
}
pub(super) fn visibility_callback(rect: egui::Rect) -> egui::PaintCallback {
    egui_wgpu::Callback::new_paint_callback(rect, Visibility)
}

impl egui_wgpu::CallbackTrait for Callback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        screen: &egui_wgpu::ScreenDescriptor,
        encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if resources.get::<gpu::Resources>().is_none() {
            resources.insert(gpu::Resources::new(device));
        }
        let format = resources
            .get::<super::contour_pass::ContourPassResources>()
            .map_or(wgpu::TextureFormat::Rgba8Unorm, |r| r.format);
        let res = resources.get_mut::<gpu::Resources>().unwrap();
        res.sync_generation(GENERATION.load(Ordering::Relaxed));
        res.set_format(device, format);
        res.prepare(device, queue, encoder, self, screen.pixels_per_point);
        vec![]
    }
    fn paint(
        &self,
        _: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &egui_wgpu::CallbackResources,
    ) {
        if let Some(res) = resources.get::<gpu::Resources>() {
            res.paint(pass, self.slot);
        }
    }
}

#[cfg(test)]
#[path = "atlas_surface_tests.rs"]
mod tests;
