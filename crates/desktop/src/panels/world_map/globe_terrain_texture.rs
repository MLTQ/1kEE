//! Cache stationary terrain; progressively restore all native elevation planes.
use super::*;

const REFINE_SEGMENTS_PER_FRAME: usize = 2_000_000;

struct Target {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    bind: wgpu::BindGroup,
}

pub(super) struct Cache {
    blit: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    size: [u32; 2],
    preview: Option<Target>,
    full: Option<Target>,
    source: Option<Arc<Prepared>>,
    uniforms: Vec<u8>,
    interval: i32,
    next_chunk: usize,
    complete: bool,
}

impl Cache {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globe terrain image layout"),
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
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor::default());
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("globe terrain image"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
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
            size: [0, 0],
            preview: None,
            full: None,
            source: None,
            uniforms: vec![],
            interval: 1,
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
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        Target {
            texture,
            view,
            bind,
        }
    }

    /// Returns whether another frame is needed to finish the all-plane image.
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
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
        let resized = size != self.size;
        if resized {
            self.size = size;
            self.preview = Some(self.target(device, format));
            self.full = Some(self.target(device, format));
        }
        let changed = resized
            || self.source.as_ref().is_none_or(|s| !Arc::ptr_eq(s, frame))
            || self.uniforms != bytemuck::bytes_of(uniforms)
            || self.interval != interval;
        if changed {
            self.source = Some(frame.clone());
            self.uniforms = bytemuck::bytes_of(uniforms).to_vec();
            self.interval = interval;
            self.next_chunk = 0;
            self.complete = false;
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
            // Small views already include every plane; their preview is final.
            self.complete = interval == 1;
            return !self.complete;
        }
        if self.complete {
            return false;
        }
        let (next, done) = render(
            encoder,
            pipeline,
            bind,
            &self.full.as_ref().unwrap().view,
            frame,
            tiles,
            viewport,
            1,
            self.next_chunk,
            REFINE_SEGMENTS_PER_FRAME,
            self.next_chunk == 0,
        );
        self.next_chunk = next;
        self.complete = done;
        !done
    }

    pub fn paint(&self, pass: &mut wgpu::RenderPass<'static>) {
        let target = if self.complete && self.interval != 1 {
            &self.full
        } else {
            &self.preview
        };
        if let Some(target) = target {
            debug_assert_eq!(target.texture.width(), self.size[0]);
            pass.set_pipeline(&self.blit);
            pass.set_bind_group(0, &target.bind, &[]);
            pass.draw(0..4, 0..1);
        }
    }
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

const SHADER: &str = r#"
@group(0) @binding(0) var terrain:texture_2d<f32>;
@group(0) @binding(1) var nearest:sampler;
struct Out { @builtin(position) position:vec4<f32>, @location(0) uv:vec2<f32> }
@vertex fn vs(@builtin(vertex_index) i:u32)->Out {
    let uv=vec2<f32>(f32(i&1u),f32(i>>1u));
    var out:Out;out.position=vec4<f32>(uv.x*2.-1.,1.-uv.y*2.,0.,1.);out.uv=uv;return out;
}
@fragment fn fs(in:Out)->@location(0) vec4<f32> {return textureSample(terrain,nearest,in.uv);}
"#;

#[cfg(test)]
#[path = "globe_terrain_texture_tests.rs"]
mod tests;
