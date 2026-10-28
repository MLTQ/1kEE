//! Bounded GPU uploads and perspective/horizon-correct spherical polygon fill.
use super::{
    contour_pass::{ContourPassResources, InstanceChunk},
    globe_scene::GlobeLayout,
    line_upload::Upload,
};
use crate::model::{GeoPoint, GlobeViewState};
use eframe::{egui_wgpu, wgpu};
use std::sync::Arc;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct Triangle {
    a: [f32; 3],
    b: [f32; 3],
    c: [f32; 3],
    color: [f32; 4],
}
impl Triangle {
    pub fn new(p: [GeoPoint; 3], color: egui::Color32) -> Self {
        let unit = |p: GeoPoint| {
            let (lat, lon) = (p.lat.to_radians(), p.lon.to_radians());
            [lat.cos() * lon.cos(), lat.sin(), lat.cos() * lon.sin()]
        };
        Self {
            a: unit(p[0]),
            b: unit(p[1]),
            c: unit(p[2]),
            color: super::globe_pass::color_to_linear(color),
        }
    }
}
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    center: [f32; 2],
    origin: [f32; 2],
    size: [f32; 2],
    rotation: [f32; 2],
    pitch: [f32; 2],
    radius_focal: f32,
    distance: f32,
    opacity: f32,
    linear_target: f32,
    _pad: [f32; 2],
}
const _: () = assert!(std::mem::size_of::<Uniforms>() == 64);
struct Resources {
    pipeline: wgpu::RenderPipeline,
    uniform: wgpu::Buffer,
    group: wgpu::BindGroup,
    used: bool,
    version: u64,
    chunks: Vec<InstanceChunk>,
    pending: Option<(u64, Upload<Triangle>)>,
}
impl Resources {
    fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("admin globe fill"),
            source: wgpu::ShaderSource::Wgsl(include_str!("admin_globe_fill.wgsl").into()),
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("admin fill uniforms"),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(64),
                },
                count: None,
            }],
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("admin spherical fill"), layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs_main"), compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout { array_stride: std::mem::size_of::<Triangle>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x3, 3 => Float32x4] }] },
            fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some("fs_main"), compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format, blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL })] }),
            primitive: Default::default(), depth_stencil: None, multisample: Default::default(), multiview: None, cache: None });
        Self {
            pipeline,
            uniform,
            group,
            used: false,
            version: 0,
            chunks: vec![],
            pending: None,
        }
    }
}
pub(super) struct Callback {
    version: u64,
    triangles: Arc<Vec<Triangle>>,
    uniforms: Uniforms,
}
impl Callback {
    pub fn callback(
        version: u64,
        triangles: Arc<Vec<Triangle>>,
        layout: &GlobeLayout,
        view: &GlobeViewState,
        opacity: f32,
        ppp: f32,
        rect: egui::Rect,
    ) -> egui::PaintCallback {
        egui_wgpu::Callback::new_paint_callback(
            rect,
            Self {
                version,
                triangles,
                uniforms: Uniforms {
                    center: [layout.center.x * ppp, layout.center.y * ppp],
                    origin: [rect.min.x * ppp, rect.min.y * ppp],
                    size: [(rect.width() * ppp).max(1.), (rect.height() * ppp).max(1.)],
                    rotation: [view.yaw.sin(), view.yaw.cos()],
                    pitch: [view.pitch.sin(), view.pitch.cos()],
                    radius_focal: layout.radius * layout.focal_length * ppp,
                    distance: layout.camera_distance,
                    opacity: (opacity.clamp(0., 0.5) * 255.0) as u8 as f32 / 255.0,
                    linear_target: 0.,
                    _pad: [0.; 2],
                },
            },
        )
    }
}
impl egui_wgpu::CallbackTrait for Callback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _: &egui_wgpu::ScreenDescriptor,
        _: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if resources.get::<Resources>().is_none() {
            let Some(contours) = resources.get::<ContourPassResources>() else {
                return vec![];
            };
            let format = contours.format;
            resources.insert(Resources::new(device, format));
        }
        let contours = resources.get_mut::<ContourPassResources>().unwrap();
        let mut budget = std::mem::take(&mut contours.uploads);
        let mut uniforms = self.uniforms;
        uniforms.linear_target = f32::from(contours.format.is_srgb());
        let res = resources.get_mut::<Resources>().unwrap();
        res.used = true;
        queue.write_buffer(&res.uniform, 0, bytemuck::bytes_of(&uniforms));
        if res.version != self.version || res.pending.is_some() {
            let (version, upload) = res
                .pending
                .get_or_insert_with(|| (self.version, Upload::new(self.triangles.clone())));
            if upload.advance(device, &mut budget) {
                res.version = *version;
                res.chunks = res.pending.take().unwrap().1.chunks;
            }
            crate::app::request_repaint();
        }
        resources.get_mut::<ContourPassResources>().unwrap().uploads = budget;
        vec![]
    }
    fn paint(
        &self,
        _: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &egui_wgpu::CallbackResources,
    ) {
        let Some(res) = resources.get::<Resources>() else {
            return;
        };
        if res.version != self.version {
            return;
        }
        pass.set_pipeline(&res.pipeline);
        pass.set_bind_group(0, &res.group, &[]);
        for chunk in &res.chunks {
            pass.set_vertex_buffer(0, chunk.buffer.slice(..));
            pass.draw(0..3, 0..chunk.count);
        }
    }
}

// Called by the canvas lifecycle around all callback prepares.
pub(super) fn begin_frame(resources: &mut egui_wgpu::CallbackResources) {
    if let Some(res) = resources.get_mut::<Resources>() {
        res.used = false;
    }
}
pub(super) fn end_frame(resources: &mut egui_wgpu::CallbackResources) {
    if let Some(res) = resources.get_mut::<Resources>()
        && !res.used
    {
        res.chunks.clear();
        res.pending = None;
        res.version = 0;
    }
}
