//! Atomic publication and camera-correct drawing of a complete globe boundary overlay.
#[path = "admin_globe_upload.rs"]
mod upload;
use super::{
    admin_globe::{Appearance, Batch},
    contour_pass::{
        ContourLayer, ContourPassResources, ContourUniforms, InstanceChunk, SegmentInstance,
    },
    globe_scene::GlobeLayout,
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
#[derive(Default)]
struct Resources {
    used: bool,
    geometry: upload::Geometry,
    fill: Option<FillPass>,
}
struct FillPass {
    pipeline: wgpu::RenderPipeline,
    uniform: wgpu::Buffer,
    group: wgpu::BindGroup,
}
impl FillPass {
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
        }
    }
}
pub(super) struct Callback {
    batch: Arc<Batch>,
    shade: bool,
    line_view: ContourUniforms,
    uniforms: Uniforms,
}
impl Callback {
    pub fn callback(
        batch: Arc<Batch>,
        layout: &GlobeLayout,
        view: &GlobeViewState,
        shade: bool,
        opacity: f32,
        layer_opacity: f32,
        painter: &egui::Painter,
    ) -> egui::PaintCallback {
        let rect = painter.clip_rect();
        let ppp = painter.ctx().pixels_per_point();
        let mut line_view = ContourUniforms::for_view(
            ContourLayer::Admin,
            layout,
            view,
            0.0,
            layer_opacity,
            1.2,
            ppp,
        );
        line_view.set_viewport(rect);
        egui_wgpu::Callback::new_paint_callback(
            rect,
            Self {
                batch,
                shade,
                line_view,
                uniforms: Uniforms {
                    center: [layout.center.x * ppp, layout.center.y * ppp],
                    origin: [rect.min.x * ppp, rect.min.y * ppp],
                    size: [(rect.width() * ppp).max(1.), (rect.height() * ppp).max(1.)],
                    rotation: [view.yaw.sin(), view.yaw.cos()],
                    pitch: [view.pitch.sin(), view.pitch.cos()],
                    radius_focal: layout.radius * layout.focal_length * ppp,
                    distance: layout.camera_distance,
                    opacity: ((opacity * layer_opacity).clamp(0., 0.5) * 255.0) as u8 as f32
                        / 255.0,
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
        let Some(contours) = resources.get_mut::<ContourPassResources>() else {
            return vec![];
        };
        contours.write_view(queue, ContourLayer::Admin, &self.line_view);
        let format = contours.format;
        let mut budget = std::mem::take(&mut contours.uploads);
        if resources.get::<Resources>().is_none() {
            resources.insert(Resources::default());
        }
        let res = resources.get_mut::<Resources>().unwrap();
        res.used = true;
        if self.shade {
            let fill = res
                .fill
                .get_or_insert_with(|| FillPass::new(device, format));
            let mut uniforms = self.uniforms;
            uniforms.linear_target = f32::from(format.is_srgb());
            queue.write_buffer(&fill.uniform, 0, bytemuck::bytes_of(&uniforms));
        }
        if !res
            .geometry
            .stage(device, &mut budget, &self.batch, self.shade)
        {
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
        let Some(displayed) = &res.geometry.displayed else {
            return;
        };
        if let (Some(fill), Some(pipeline)) = (&displayed.fill, &res.fill) {
            pass.set_pipeline(&pipeline.pipeline);
            pass.set_bind_group(0, &pipeline.group, &[]);
            for chunk in fill {
                pass.set_vertex_buffer(0, chunk.buffer.slice(..));
                pass.draw(0..3, 0..chunk.count);
            }
        }
        if let Some(contours) = resources.get::<ContourPassResources>() {
            contours.draw_chunks(pass, ContourLayer::Admin, &displayed.lines);
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
        res.geometry = upload::Geometry::default();
    }
}
