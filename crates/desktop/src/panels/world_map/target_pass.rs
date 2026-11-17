//! Target point sprites and measured terrain with their own depth attachment.
use crate::target_mode::{Batch, ContextGeometry, Terrain, Vertex, camera::Camera};
use eframe::{egui_wgpu, wgpu};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use wgpu::util::DeviceExt;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    eye: [f32; 4],
    right: [f32; 4],
    up: [f32; 4],
    forward: [f32; 4],
    projection: [f32; 4],
    viewport: [f32; 4],
    flags: [u32; 4],
    height: [f32; 4],
    light: [f32; 4],
}
#[derive(Clone)]
pub(crate) struct Draw {
    pub session: u64,
    pub camera: Camera,
    pub radius: f32,
    pub clouds: Vec<Arc<Batch>>,
    pub terrain: Option<Arc<Terrain>>,
    pub context: Option<Arc<ContextGeometry>>,
    pub points: bool,
    pub surface: bool,
    pub context_visible: bool,
    pub class_mask: u32,
    pub color_mode: u32,
    pub point_size: f32,
    pub light: [f32; 4],
    pub rect: egui::Rect,
    pub ctx: egui::Context,
}
struct GpuBatch {
    chunks: Vec<(wgpu::Buffer, u32)>,
    uploaded: usize,
}
struct TargetTexture {
    size: [u32; 2],
    _color: wgpu::Texture,
    view: wgpu::TextureView,
    _depth: wgpu::Texture,
    depth: wgpu::TextureView,
    bind: wgpu::BindGroup,
}
pub(crate) struct Resources {
    point: wgpu::RenderPipeline,
    surface: wgpu::RenderPipeline,
    lines: wgpu::RenderPipeline,
    blit: wgpu::RenderPipeline,
    uniform: wgpu::Buffer,
    bind: wgpu::BindGroup,
    texture_layout: wgpu::BindGroupLayout,
    texture: Option<TargetTexture>,
    batches: HashMap<u64, GpuBatch>,
    indices: Option<(u64, wgpu::Buffer, u32)>,
    session: u64,
}
const ATTRS: [wgpu::VertexAttribute; 3] =
    wgpu::vertex_attr_array![0=>Float32x3,1=>Unorm8x4,2=>Uint32];
impl Resources {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Target measured geometry"),
            source: wgpu::ShaderSource::Wgsl(include_str!("target_points.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Target camera"),
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
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Target camera"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Target camera"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Target scene"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let pipeline = |point: bool, topology, bias| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Target depth-tested geometry"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(if point { "vs_point" } else { "vs_surface" }),
                    compilation_options: Default::default(),
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<Vertex>() as u64,
                        step_mode: if point {
                            wgpu::VertexStepMode::Instance
                        } else {
                            wgpu::VertexStepMode::Vertex
                        },
                        attributes: &ATTRS,
                    }],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(if point {
                        "fs_point"
                    } else if topology == wgpu::PrimitiveTopology::LineList {
                        "fs_lines"
                    } else {
                        "fs_surface"
                    }),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: wgpu::TextureFormat::Rgba8UnormSrgb,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState {
                    topology,
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: true,
                    depth_compare: wgpu::CompareFunction::GreaterEqual,
                    stencil: Default::default(),
                    bias: wgpu::DepthBiasState {
                        constant: bias,
                        ..Default::default()
                    },
                }),
                multisample: Default::default(),
                multiview: None,
                cache: None,
            })
        };
        #[cfg(test)]
        eprintln!("Target compiling point pipeline");
        let point = pipeline(true, wgpu::PrimitiveTopology::TriangleList, 0);
        #[cfg(test)]
        eprintln!("Target compiling surface pipeline");
        let surface = pipeline(false, wgpu::PrimitiveTopology::TriangleList, 0);
        #[cfg(test)]
        eprintln!("Target compiling line pipeline");
        let lines = pipeline(false, wgpu::PrimitiveTopology::LineList, 1);
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Target image"),
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
        let blit_shader=device.create_shader_module(wgpu::ShaderModuleDescriptor {label:Some("Target composite"),source:wgpu::ShaderSource::Wgsl(r#"
            @group(0) @binding(0) var image: texture_2d<f32>;
            @group(0) @binding(1) var filtering: sampler;
            struct Out {@builtin(position) position:vec4<f32>,@location(0) uv:vec2<f32>}
            @vertex fn vs(@builtin(vertex_index) i:u32)->Out {
                let uv=vec2<f32>(f32((i << 1u) & 2u),f32(i & 2u));
                var o:Out;o.position=vec4<f32>(uv * 2. - 1.,0.,1.);o.uv=vec2<f32>(uv.x,1.-uv.y);return o;
            }
            @fragment fn fs(i:Out)->@location(0) vec4<f32> {return textureSample(image,filtering,i.uv);}
            @fragment fn fs_gamma(i:Out)->@location(0) vec4<f32> {
                let c=textureSample(image,filtering,i.uv);
                let rgb=select(1.055*pow(c.rgb,vec3<f32>(1./2.4))-0.055,c.rgb*12.92,c.rgb<=vec3<f32>(0.0031308));
                return vec4<f32>(rgb,c.a);
            }
        "#.into())});
        let blit_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[&texture_layout],
            push_constant_ranges: &[],
        });
        #[cfg(test)]
        eprintln!("Target compiling composite pipeline");
        let blit = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Target composite"),
            layout: Some(&blit_layout),
            vertex: wgpu::VertexState {
                module: &blit_shader,
                entry_point: Some("vs"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &blit_shader,
                entry_point: Some(if format.is_srgb() { "fs" } else { "fs_gamma" }),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });
        Self {
            point,
            surface,
            lines,
            blit,
            uniform,
            bind,
            texture_layout,
            texture: None,
            batches: HashMap::new(),
            indices: None,
            session: 0,
        }
    }
    fn ensure_texture(&mut self, device: &wgpu::Device, size: [u32; 2]) {
        if self.texture.as_ref().is_some_and(|t| t.size == size) {
            return;
        }
        let desc = |format, usage| wgpu::TextureDescriptor {
            label: Some("Target offscreen"),
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        };
        let color = device.create_texture(&desc(
            wgpu::TextureFormat::Rgba8UnormSrgb,
            wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC,
        ));
        let view = color.create_view(&Default::default());
        let depth_texture = device.create_texture(&desc(
            wgpu::TextureFormat::Depth32Float,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
        ));
        let depth = depth_texture.create_view(&Default::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Target image"),
            layout: &self.texture_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });
        self.texture = Some(TargetTexture {
            size,
            _color: color,
            view,
            _depth: depth_texture,
            depth,
            bind,
        });
    }
    fn upload(&mut self, device: &wgpu::Device, source: &Batch, budget: &mut usize) {
        let batch = self.batches.entry(source.id).or_insert_with(|| GpuBatch {
            chunks: Vec::new(),
            uploaded: 0,
        });
        let stride = std::mem::size_of::<Vertex>();
        let max = ((*budget).min(device.limits().max_buffer_size as usize) / stride)
            .min(8 * 1024 * 1024 / stride);
        while batch.uploaded < source.points.len() && *budget >= stride && max > 0 {
            let allowance = max.min(*budget / stride) / 2 * 2;
            if allowance == 0 {
                break;
            }
            let count = (source.points.len() - batch.uploaded).min(allowance);
            let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Target measured point chunk"),
                contents: bytemuck::cast_slice(
                    &source.points[batch.uploaded..batch.uploaded + count],
                ),
                usage: wgpu::BufferUsages::VERTEX,
            });
            batch.chunks.push((buffer, count as u32));
            batch.uploaded += count;
            *budget -= count * stride;
        }
    }
    fn prepare(
        &mut self,
        draw: &Draw,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> wgpu::CommandBuffer {
        if self.session != draw.session {
            self.batches.clear();
            self.indices = None;
            self.session = draw.session;
        }
        let ppp = draw.ctx.pixels_per_point();
        let maximum = device.limits().max_texture_dimension_2d.min(4096);
        let physical = draw.rect.size() * ppp;
        let resolution_scale = (maximum as f32 / physical.x.max(physical.y).max(1.)).min(1.);
        let size = [
            (physical.x * resolution_scale)
                .round()
                .clamp(1., maximum as f32) as u32,
            (physical.y * resolution_scale)
                .round()
                .clamp(1., maximum as f32) as u32,
        ];
        self.ensure_texture(device, size);
        let sources = draw
            .terrain
            .iter()
            .map(|t| &t.batch)
            .chain(draw.context.iter().map(|c| &c.batch))
            .chain(draw.clouds.iter())
            .collect::<Vec<_>>();
        let keep = sources.iter().map(|b| b.id).collect::<HashSet<_>>();
        self.batches.retain(|id, _| keep.contains(id));
        let mut budget = 8 * 1024 * 1024;
        for batch in &sources {
            self.upload(device, batch, &mut budget);
        }
        if sources.iter().any(|b| {
            self.batches
                .get(&b.id)
                .is_none_or(|g| g.uploaded < b.points.len())
        }) {
            draw.ctx.request_repaint();
        }
        if let Some(terrain) = &draw.terrain
            && self
                .indices
                .as_ref()
                .is_none_or(|i| i.0 != terrain.batch.id)
        {
            self.indices = Some((
                terrain.batch.id,
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Target terrain indices"),
                    contents: bytemuck::cast_slice(&terrain.indices),
                    usage: wgpu::BufferUsages::INDEX,
                }),
                terrain.indices.len() as u32,
            ));
        }
        let (right, up, forward) = draw.camera.basis();
        let v4 = |p: [f32; 3]| [p[0], p[1], p[2], 0.];
        let ground = if draw.points {
            draw.clouds
                .first()
                .and_then(|b| b.points.first())
                .map(|p| p.position[2])
        } else {
            draw.terrain.as_ref().map(|t| t.center_height())
        }
        .unwrap_or(draw.camera.eye[2] - 1100.);
        let uniforms = Uniforms {
            eye: v4(draw.camera.eye),
            right: v4(right),
            up: v4(up),
            forward: v4(forward),
            projection: [
                (60f32.to_radians() / 2.).tan(),
                size[0] as f32 / size[1] as f32,
                0.1,
                30_000.,
            ],
            viewport: [
                size[0] as f32,
                size[1] as f32,
                draw.point_size * ppp * resolution_scale,
                draw.radius,
            ],
            flags: [draw.class_mask, draw.color_mode, 0, 0],
            height: [ground, 150., 0., 0.],
            light: draw.light,
        };
        queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&uniforms));
        let texture = self.texture.as_ref().unwrap();
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Target scene"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Target depth-tested scene"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &texture.view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.006,
                            g: 0.012,
                            b: 0.018,
                            a: 1.,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &texture.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(0.),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                occlusion_query_set: None,
                timestamp_writes: None,
            });
            pass.set_bind_group(0, &self.bind, &[]);
            if draw.surface
                && let (Some(terrain), Some((id, indices, count))) = (&draw.terrain, &self.indices)
                && *id == terrain.batch.id
                && let Some(batch) = self.batches.get(id)
                && batch.uploaded == terrain.batch.points.len()
                && batch.chunks.len() == 1
            {
                pass.set_pipeline(&self.surface);
                pass.set_vertex_buffer(0, batch.chunks[0].0.slice(..));
                pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..*count, 0, 0..1);
            }
            if draw.points {
                pass.set_pipeline(&self.point);
                for source in &draw.clouds {
                    if let Some(batch) = self.batches.get(&source.id) {
                        for (buffer, count) in &batch.chunks {
                            pass.set_vertex_buffer(0, buffer.slice(..));
                            pass.draw(0..6, 0..*count);
                        }
                    }
                }
            }
            if draw.context_visible
                && let Some(context) = &draw.context
                && let Some(batch) = self.batches.get(&context.batch.id)
            {
                pass.set_pipeline(&self.lines);
                for (buffer, count) in &batch.chunks {
                    pass.set_vertex_buffer(0, buffer.slice(..));
                    pass.draw(0..*count, 0..1);
                }
            }
        }
        encoder.finish()
    }
}
impl egui_wgpu::CallbackTrait for Draw {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(pass) = resources.get_mut::<Resources>() else {
            return Vec::new();
        };
        vec![pass.prepare(self, device, queue)]
    }
    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &egui_wgpu::CallbackResources,
    ) {
        if let Some(resource) = resources.get::<Resources>()
            && let Some(texture) = &resource.texture
        {
            pass.set_pipeline(&resource.blit);
            pass.set_bind_group(0, &texture.bind, &[]);
            pass.draw(0..3, 0..1);
        }
    }
}
pub(crate) fn release(rect: egui::Rect) -> egui::Shape {
    struct Release;
    impl egui_wgpu::CallbackTrait for Release {
        fn prepare(
            &self,
            _d: &wgpu::Device,
            _q: &wgpu::Queue,
            _s: &egui_wgpu::ScreenDescriptor,
            _e: &mut wgpu::CommandEncoder,
            resources: &mut egui_wgpu::CallbackResources,
        ) -> Vec<wgpu::CommandBuffer> {
            if let Some(p) = resources.get_mut::<Resources>() {
                p.batches.clear();
                p.indices = None;
                p.texture = None;
                p.session = 0;
            }
            Vec::new()
        }
        fn paint(
            &self,
            _i: egui::PaintCallbackInfo,
            _p: &mut wgpu::RenderPass<'static>,
            _r: &egui_wgpu::CallbackResources,
        ) {
        }
    }
    egui_wgpu::Callback::new_paint_callback(rect, Release).into()
}

#[cfg(test)]
#[path = "target_gpu_tests.rs"]
mod tests;
