//! Shared offscreen color + Reversed-Z Depth32Float render target for all
//! unified 3D map passes (Atlas terrain, RTC vector lines, LiDAR point cloud).
//!
//! Within a single egui frame, `BeginSceneCallback` clears `(color, depth)` in
//! `prepare()`, each 3D pass draws into the shared target in `prepare()` using
//! `LoadOp::Load` and `CompareFunction::GreaterEqual`, and `CompositeSceneCallback`
//! blits the depth-composited color buffer onto egui's render pass in `paint()`.

use eframe::{egui_wgpu, wgpu};
use std::collections::HashMap;

pub const SCENE_COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
pub const SCENE_DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

struct Target {
    size: [u32; 2],
    _color: wgpu::Texture,
    color_view: wgpu::TextureView,
    _depth: wgpu::Texture,
    depth_view: wgpu::TextureView,
    blit_bind: wgpu::BindGroup,
}

pub struct UnifiedSceneTarget {
    blit_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    blit_pipelines: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
    target: Option<Target>,
    target_format: wgpu::TextureFormat,
}

impl UnifiedSceneTarget {
    pub fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        let blit_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("unified_scene_blit_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("unified_scene_blit_sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let mut me = Self {
            blit_layout,
            sampler,
            blit_pipelines: HashMap::new(),
            target: None,
            target_format,
        };
        me.ensure_blit_pipeline(device, target_format);
        me
    }

    fn ensure_blit_pipeline(&mut self, device: &wgpu::Device, format: wgpu::TextureFormat) {
        if self.blit_pipelines.contains_key(&format) {
            return;
        }
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("unified_scene_blit_shader"),
            source: super::gpu_color::shader(BLIT_WGSL),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("unified_scene_blit_pl"),
            bind_group_layouts: &[&self.blit_layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("unified_scene_blit_pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: super::gpu_color::fragment_options(format),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
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
        self.blit_pipelines.insert(format, pipeline);
    }

    pub fn ensure_size(&mut self, device: &wgpu::Device, size: [u32; 2]) {
        let size = [size[0].max(1), size[1].max(1)];
        if self.target.as_ref().is_some_and(|t| t.size == size) {
            return;
        }
        let color = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("unified_scene_color"),
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: SCENE_COLOR_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let depth = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("unified_scene_depth_revz"),
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: SCENE_DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let color_view = color.create_view(&Default::default());
        let depth_view = depth.create_view(&Default::default());
        let blit_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("unified_scene_blit_bg"),
            layout: &self.blit_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&color_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        self.target = Some(Target {
            size,
            _color: color,
            color_view,
            _depth: depth,
            depth_view,
            blit_bind,
        });
    }

    pub fn views(&self) -> Option<(&wgpu::TextureView, &wgpu::TextureView, [u32; 2])> {
        let t = self.target.as_ref()?;
        Some((&t.color_view, &t.depth_view, t.size))
    }

    pub fn clear(&self, encoder: &mut wgpu::CommandEncoder) {
        let Some(t) = self.target.as_ref() else {
            return;
        };
        let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("unified_scene_clear"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &t.color_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &t.depth_view,
                depth_ops: Some(wgpu::Operations {
                    // Reversed-Z: 0.0 is far plane, 1.0 is near plane.
                    load: wgpu::LoadOp::Clear(0.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });
    }

    pub fn paint_composite(&self, pass: &mut wgpu::RenderPass<'static>) {
        let Some(t) = self.target.as_ref() else {
            return;
        };
        let Some(pipeline) = self.blit_pipelines.get(&self.target_format) else {
            return;
        };
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &t.blit_bind, &[]);
        pass.draw(0..3, 0..1);
    }
}

pub struct BeginSceneCallback {
    pub rect: egui::Rect,
}

impl BeginSceneCallback {
    pub fn into_paint_callback(self) -> egui::PaintCallback {
        egui_wgpu::Callback::new_paint_callback(self.rect, self)
    }
}

impl egui_wgpu::CallbackTrait for BeginSceneCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        _queue: &wgpu::Queue,
        screen: &egui_wgpu::ScreenDescriptor,
        encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let max_side = device.limits().max_texture_dimension_2d.min(8192);
        let w = ((self.rect.width() * screen.pixels_per_point).round() as u32).clamp(1, max_side);
        let h = ((self.rect.height() * screen.pixels_per_point).round() as u32).clamp(1, max_side);
        if let Some(scene) = resources.get_mut::<UnifiedSceneTarget>() {
            scene.ensure_size(device, [w, h]);
            scene.clear(encoder);
        }
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        _render_pass: &mut wgpu::RenderPass<'static>,
        _resources: &egui_wgpu::CallbackResources,
    ) {
    }
}

pub struct CompositeSceneCallback {
    pub rect: egui::Rect,
}

impl CompositeSceneCallback {
    pub fn into_paint_callback(self) -> egui::PaintCallback {
        egui_wgpu::Callback::new_paint_callback(self.rect, self)
    }
}

impl egui_wgpu::CallbackTrait for CompositeSceneCallback {
    fn prepare(
        &self,
        _device: &wgpu::Device,
        _queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        _resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        resources: &egui_wgpu::CallbackResources,
    ) {
        if let Some(scene) = resources.get::<UnifiedSceneTarget>() {
            scene.paint_composite(render_pass);
        }
    }
}

const BLIT_WGSL: &str = r#"
@group(0) @binding(0) var scene_tex: texture_2d<f32>;
@group(0) @binding(1) var scene_sampler: sampler;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
    var verts = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 3.0, -1.0),
        vec2<f32>(-1.0,  3.0),
    );
    let p = verts[vi];
    var out: VsOut;
    out.pos = vec4<f32>(p, 0.0, 1.0);
    out.uv = vec2<f32>(p.x * 0.5 + 0.5, 0.5 - p.y * 0.5);
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let c = textureSample(scene_tex, scene_sampler, in.uv);
    return map_output_gamma(c);
}
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panels::world_map::{
        point_cloud_pass::PointCloudResources,
        unified_globe_pass::UnifiedGlobePassResources,
        unified_line_pass::UnifiedLineResources,
        unified_terrain_pass::UnifiedTerrainResources,
    };

    #[test]
    fn unified_gpu_pipelines_and_wgsl_compile_cleanly() {
        let instance = wgpu::Instance::default();
        let adapter = pollster_block(instance.request_adapter(&wgpu::RequestAdapterOptions::default()));
        let Some(adapter) = adapter else {
            return;
        };
        let Ok((device, _queue)) = pollster_block(adapter.request_device(&wgpu::DeviceDescriptor::default(), None)) else {
            return;
        };
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let _scene = UnifiedSceneTarget::new(&device, format);
        let _globe = UnifiedGlobePassResources::new(&device, format);
        let _lines = UnifiedLineResources::new(&device);
        let _terrain = UnifiedTerrainResources::new(&device);
        let _points = PointCloudResources::new(&device);
    }

    fn pollster_block<F: std::future::Future>(fut: F) -> F::Output {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(fut)
    }
}
