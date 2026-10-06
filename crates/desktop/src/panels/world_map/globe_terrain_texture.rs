//! Reproject retained detail while bounded full-resolution snapshots refresh.
use super::*;
const REFINE_SEGMENTS_PER_FRAME: usize = 2_000_000;
struct Target {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}
struct Refinement {
    source: Arc<Prepared>,
    uniforms: ContourUniforms,
    viewport: Viewport,
    bind: wgpu::BindGroup,
    target: Target,
    next: usize,
}
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Warp {
    current: ContourUniforms,
    saved: ContourUniforms,
    flags: [f32; 4],
}

pub(super) struct Cache {
    blit: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    warp: wgpu::Buffer,
    paint_bind: Option<wgpu::BindGroup>,
    size: [u32; 2],
    preview: Option<Target>,
    full: Option<Target>,
    spare: Option<Target>,
    source: Option<Arc<Prepared>>,
    uniforms: Option<ContourUniforms>,
    full_source: Option<Arc<Prepared>>,
    full_uniforms: Option<ContourUniforms>,
    working: Option<Refinement>,
    next_chunk: usize,
    complete: bool,
}
impl Cache {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let texture = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("terrain reprojection"),
            entries: &[
                texture(0),
                texture(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let warp = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("terrain image cameras"),
            size: std::mem::size_of::<Warp>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("globe terrain reprojection"),
            source: wgpu::ShaderSource::Wgsl(include_str!("globe_terrain_reproject.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let blit = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("cached globe terrain"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });
        Self {
            blit,
            layout,
            sampler,
            warp,
            paint_bind: None,
            size: [0, 0],
            preview: None,
            full: None,
            spare: None,
            source: None,
            uniforms: None,
            full_source: None,
            full_uniforms: None,
            working: None,
            next_chunk: 0,
            complete: false,
        }
    }
    fn target(&self, device: &wgpu::Device, format: wgpu::TextureFormat) -> Target {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("cached globe terrain"),
            size: wgpu::Extent3d {
                width: self.size[0],
                height: self.size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        Target { texture, view }
    }
    pub fn retained_versions(&self) -> impl Iterator<Item = u64> + '_ {
        self.working
            .iter()
            .flat_map(|w| w.source.tiles.iter().map(|t| t.version))
    }
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        format: wgpu::TextureFormat,
        pipeline: &wgpu::RenderPipeline,
        bind: &wgpu::BindGroup,
        frame: &Arc<Prepared>,
        tiles: &HashMap<u64, Vec<InstanceChunk>>,
        uniforms: &ContourUniforms,
        viewport: Viewport,
        interval: i32,
    ) -> bool {
        let size = uniforms.viewport_size.map(|v| v.round().max(1.) as u32);
        let resized = self.size != size;
        let incompatible = self.uniforms.is_some_and(|u| !same_style(&u, uniforms))
            || self.source.as_ref().is_some_and(|old| {
                old.palette != frame.palette
                    || (!Arc::ptr_eq(old, frame)
                        && !old
                            .tiles
                            .iter()
                            .any(|a| frame.tiles.iter().any(|b| Arc::ptr_eq(a, b))))
            });
        if resized || incompatible {
            self.size = size;
            self.full = None;
            self.full_source = None;
            self.full_uniforms = None;
            self.working = None;
            self.spare = None;
            self.uniforms = None;
            self.preview = Some(self.target(device, format));
        }
        let changed = self.source.as_ref().is_none_or(|s| !Arc::ptr_eq(s, frame))
            || self
                .uniforms
                .as_ref()
                .is_none_or(|u| !same_camera(u, uniforms));
        if !changed && self.complete {
            return false;
        }
        if changed {
            self.source = Some(frame.clone());
            self.uniforms = Some(*uniforms);
            render(
                encoder,
                pipeline,
                bind,
                &self.preview.as_ref().unwrap().view,
                frame,
                tiles,
                viewport,
                interval,
                0,
                usize::MAX,
                true,
            );
        }
        self.complete = self
            .full_source
            .as_ref()
            .is_some_and(|s| Arc::ptr_eq(s, frame))
            && self
                .full_uniforms
                .as_ref()
                .is_some_and(|u| same_camera(u, uniforms));
        if !self.complete && self.working.is_none() {
            // Freeze a complete camera/source snapshot. Movement must not
            // restart it, or a continuously moving globe can never refine.
            let mut bytes =
                vec![0; UNIFORM_STRIDE as usize * (ContourLayer::SrtmGlobe.slot() as usize + 1)];
            let offset = ContourLayer::SrtmGlobe.slot() as usize * UNIFORM_STRIDE as usize;
            bytes[offset..offset + std::mem::size_of::<ContourUniforms>()]
                .copy_from_slice(bytemuck::bytes_of(uniforms));
            let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("frozen terrain camera"),
                contents: &bytes,
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let frozen_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &pipeline.get_bind_group_layout(0),
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &buffer,
                        offset: 0,
                        size: wgpu::BufferSize::new(std::mem::size_of::<ContourUniforms>() as u64),
                    }),
                }],
            });
            let target = self
                .spare
                .take()
                .unwrap_or_else(|| self.target(device, format));
            self.working = Some(Refinement {
                source: frame.clone(),
                uniforms: *uniforms,
                viewport,
                bind: frozen_bind,
                target,
                next: 0,
            });
        }
        if let Some(job) = self.working.as_mut() {
            let (next, done) = render(
                encoder,
                pipeline,
                &job.bind,
                &job.target.view,
                &job.source,
                tiles,
                job.viewport,
                1,
                job.next,
                REFINE_SEGMENTS_PER_FRAME,
                job.next == 0,
            );
            job.next = next;
            self.next_chunk = next;
            if done {
                let job = self.working.take().unwrap();
                self.spare = self.full.replace(job.target);
                self.complete =
                    Arc::ptr_eq(&job.source, frame) && same_camera(&job.uniforms, uniforms);
                self.full_source = Some(job.source);
                self.full_uniforms = Some(job.uniforms);
            }
        }
        let saved = self.full_uniforms.unwrap_or(*uniforms);
        let warp = Warp {
            current: *uniforms,
            saved,
            flags: [
                if self.full.is_some() { 1. } else { 0. },
                if same_camera(&saved, uniforms) {
                    1.
                } else {
                    0.
                },
                0.,
                0.,
            ],
        };
        queue.write_buffer(&self.warp, 0, bytemuck::bytes_of(&warp));
        let preview = self.preview.as_ref().unwrap();
        let full = self.full.as_ref().unwrap_or(preview);
        self.paint_bind = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&preview.view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&full.view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.warp.as_entire_binding(),
                },
            ],
        }));
        !self.complete
    }
    pub fn paint(&self, pass: &mut wgpu::RenderPass<'static>) {
        if let Some(bind) = &self.paint_bind {
            pass.set_pipeline(&self.blit);
            pass.set_bind_group(0, bind, &[]);
            pass.draw(0..4, 0..1);
        }
    }
}
fn same_camera(a: &ContourUniforms, b: &ContourUniforms) -> bool {
    bytemuck::bytes_of(a) == bytemuck::bytes_of(b)
}
fn same_style(a: &ContourUniforms, b: &ContourUniforms) -> bool {
    a.alpha == b.alpha
        && a.stroke_half_px == b.stroke_half_px
        && a.feather_px == b.feather_px
        && a.radius_offset == b.radius_offset
        && a.horizon_z == b.horizon_z
}

fn render(
    encoder: &mut wgpu::CommandEncoder,
    pipeline: &wgpu::RenderPipeline,
    bind: &wgpu::BindGroup,
    target: &wgpu::TextureView,
    frame: &Prepared,
    tiles: &HashMap<u64, Vec<InstanceChunk>>,
    viewport: Viewport,
    interval: i32,
    start: usize,
    budget: usize,
    clear: bool,
) -> (usize, bool) {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("native terrain image refinement"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target,
            resolve_target: None,
            ops: wgpu::Operations {
                load: if clear {
                    wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT)
                } else {
                    wgpu::LoadOp::Load
                },
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(
        0,
        bind,
        &[ContourLayer::SrtmGlobe.slot() * UNIFORM_STRIDE as u32],
    );
    let mut drawn = 0;
    let mut next = start;
    for (index, (chunk, gpu)) in frame
        .tiles
        .iter()
        .flat_map(|t| t.chunks.iter().zip(&tiles[&t.version]))
        .enumerate()
        .skip(start)
    {
        if viewport.intersects(chunk.bounds) {
            if drawn > 0 && drawn + gpu.count as usize > budget {
                return (index, false);
            }
            pass.set_vertex_buffer(0, gpu.buffer.slice(..));
            density::ranges(&chunk.planes, gpu.count, interval, |range| {
                pass.draw(0..4, range)
            });
            drawn += gpu.count as usize;
        }
        next = index + 1;
    }
    (next, true)
}

#[cfg(test)]
#[path = "globe_terrain_texture_tests.rs"]
mod tests;
