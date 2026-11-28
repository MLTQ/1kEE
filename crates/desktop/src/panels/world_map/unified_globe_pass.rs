//! Precision-stable RTE ray–sphere backdrop for the unified continuous camera.
//!
//! Renders Earth, Moon, and Mars in both Perspective and Orthographic modes
//! continuously from orbital space down to metre-level zoom without `f32`
//! precision breakdown.

use super::globe_pass::color_to_linear;
use super::projection::MapProjection;
use crate::model::ActiveBody;
use eframe::{egui_wgpu, wgpu};

/// Byte-for-byte mirror of `Uniforms` in `unified_globe.wgsl` (224 bytes).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct UnifiedGlobeUniforms {
    pub cam_right: [f32; 4],
    pub cam_up: [f32; 4],
    pub cam_forward: [f32; 4],
    pub rel_eye: [f32; 4],
    pub target_normal: [f32; 4],
    pub view_span: [f32; 4],
    pub mode_ctrl: [f32; 4],
    pub ocean_col: [f32; 4],
    pub land_col: [f32; 4],
    pub mount_col: [f32; 4],
    pub grid_col: [f32; 4],
    pub hot_col: [f32; 4],
    pub show_graticule: u32,
    pub active_body: u32,
    pub atlas: u32,
    pub _pad: u32,
    pub radius: f32,
    pub _pad1: f32,
    pub _pad2: f32,
    pub _pad3: f32,
}

const _: () = assert!(std::mem::size_of::<UnifiedGlobeUniforms>() == 224);

pub struct UnifiedGlobePassResources {
    pipeline: wgpu::RenderPipeline,
    uniform_buf: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

impl UnifiedGlobePassResources {
    pub fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("unified_globe_shader"),
            source: super::gpu_color::shader(UNIFIED_GLOBE_WGSL),
        });
        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("unified_globe_uniforms"),
            size: std::mem::size_of::<UnifiedGlobeUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("unified_globe_bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("unified_globe_bg"),
            layout: &bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buf.as_entire_binding(),
            }],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("unified_globe_pl"),
            bind_group_layouts: &[&bgl],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("unified_globe_pipeline"),
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: super::gpu_color::fragment_options(target_format),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });
        Self {
            pipeline,
            uniform_buf,
            bind_group,
        }
    }
}

pub struct UnifiedGlobeCallback {
    uniforms: UnifiedGlobeUniforms,
    rect: egui::Rect,
}

impl UnifiedGlobeCallback {
    #[allow(clippy::too_many_arguments)]
    pub fn from_projection(
        proj: &MapProjection,
        ocean_col: egui::Color32,
        land_col: egui::Color32,
        mount_col: egui::Color32,
        grid_col: egui::Color32,
        hot_col: egui::Color32,
    ) -> Self {
        let target_sea = proj.target_up * proj.radius_m;
        let rel_eye = proj.eye_ecef - target_sea;
        let is_ortho = proj.projection.is_ortho();
        let (span_x, span_y) = if is_ortho {
            (proj.half_w_m as f32, proj.half_h_m as f32)
        } else {
            (
                (proj.tan_half_fov * proj.aspect) as f32,
                proj.tan_half_fov as f32,
            )
        };
        let approx_px_radius = (proj.radius_m / proj.meters_per_point().max(1e-3)) as f32;
        Self {
            rect: proj.viewport,
            uniforms: UnifiedGlobeUniforms {
                cam_right: [
                    proj.cam_right.x as f32,
                    proj.cam_right.y as f32,
                    proj.cam_right.z as f32,
                    proj.viewport.min.x,
                ],
                cam_up: [
                    proj.cam_up.x as f32,
                    proj.cam_up.y as f32,
                    proj.cam_up.z as f32,
                    proj.viewport.min.y,
                ],
                cam_forward: [
                    proj.cam_forward.x as f32,
                    proj.cam_forward.y as f32,
                    proj.cam_forward.z as f32,
                    proj.pixels_per_point,
                ],
                rel_eye: [
                    rel_eye.x as f32,
                    rel_eye.y as f32,
                    rel_eye.z as f32,
                    proj.radius_m as f32,
                ],
                target_normal: [
                    proj.target_up.x as f32,
                    proj.target_up.y as f32,
                    proj.target_up.z as f32,
                    (1.0 / proj.radius_m) as f32,
                ],
                view_span: [span_x, span_y, proj.viewport.width(), proj.viewport.height()],
                mode_ctrl: [
                    if is_ortho { 1.0 } else { 0.0 },
                    1.0,
                    proj.range_m as f32,
                    0.0,
                ],
                ocean_col: color_to_linear(ocean_col),
                land_col: color_to_linear(land_col),
                mount_col: color_to_linear(mount_col),
                grid_col: color_to_linear(grid_col),
                hot_col: color_to_linear(hot_col),
                show_graticule: 0,
                active_body: match proj.body {
                    ActiveBody::Earth => 0,
                    ActiveBody::Moon => 1,
                    ActiveBody::Mars => 2,
                },
                atlas: crate::theme::is_atlas() as u32,
                _pad: 0,
                radius: approx_px_radius,
                _pad1: 0.0,
                _pad2: 0.0,
                _pad3: 0.0,
            },
        }
    }

    pub fn into_paint_callback(self) -> egui::PaintCallback {
        egui_wgpu::Callback::new_paint_callback(self.rect, self)
    }
}

impl egui_wgpu::CallbackTrait for UnifiedGlobeCallback {
    fn prepare(
        &self,
        _device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if let Some(res) = resources.get::<UnifiedGlobePassResources>() {
            queue.write_buffer(&res.uniform_buf, 0, bytemuck::bytes_of(&self.uniforms));
        }
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        resources: &egui_wgpu::CallbackResources,
    ) {
        if let Some(res) = resources.get::<UnifiedGlobePassResources>() {
            render_pass.set_pipeline(&res.pipeline);
            render_pass.set_bind_group(0, &res.bind_group, &[]);
            render_pass.draw(0..3, 0..1);
        }
    }
}

const UNIFIED_GLOBE_WGSL: &str = concat!(
    include_str!("globe_surface.wgsl"),
    "\n",
    include_str!("unified_globe.wgsl")
);
