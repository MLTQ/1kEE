//! Private terrain color/depth targets and bounded publication of height maps.
use super::*;
use std::collections::{HashMap, HashSet};
use wgpu::util::DeviceExt;
#[path = "atlas_surface_forest.rs"]
mod forest;

struct Source {
    raster: Arc<Raster>,
    height: wgpu::Texture,
    color: wgpu::Texture,
    bind: wgpu::BindGroup,
    row: usize,
    forest: forest::Forest,
}
struct Target {
    _color: wgpu::Texture,
    color: wgpu::TextureView,
    _depth: wgpu::Texture,
    depth: wgpu::TextureView,
    bind: wgpu::BindGroup,
    size: [u32; 2],
}
#[derive(Default)]
struct Slot {
    sources: HashMap<u64, Source>,
    active: Vec<u64>,
    target: Option<Target>,
    key: Vec<u8>,
    coverage: Option<wgpu::Texture>,
    coverage_bytes: Vec<u8>,
}
pub(super) struct Resources {
    format: Option<wgpu::TextureFormat>,
    surface: wgpu::RenderPipeline,
    blit: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
    uniforms_layout: wgpu::BindGroupLayout,
    source_layout: wgpu::BindGroupLayout,
    blit_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    slots: [Slot; 2],
    generation: u64,
    keep_globe: bool,
    #[cfg(test)]
    uploaded_bytes: usize,
}
fn texture(
    device: &wgpu::Device,
    size: [u32; 2],
    format: wgpu::TextureFormat,
    usage: wgpu::TextureUsages,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Atlas terrain"),
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
    })
}
fn image_entry(binding: u32, filterable: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}
fn source_bind(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    height: &wgpu::Texture,
    color: &wgpu::Texture,
    forest: &wgpu::Texture,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Atlas source and forest"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(
                    &height.create_view(&Default::default()),
                ),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(
                    &color.create_view(&Default::default()),
                ),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(
                    &forest.create_view(&Default::default()),
                ),
            },
        ],
    })
}
impl Resources {
    pub fn sync_generation(&mut self, generation: u64) {
        // Switching Globe/Local/theme keeps both completed GPU views. Only an
        // explicit terrain invalidation drops them. The Globe residency
        // experiment also keeps older completed regions in prepare().
        if self.generation != generation {
            self.slots = Default::default();
            self.generation = generation;
        }
    }

    pub fn new(device: &wgpu::Device) -> Self {
        let uniforms_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let source_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[
                image_entry(0, false),
                image_entry(1, true),
                image_entry(3, true),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let blit_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[
                image_entry(0, true),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Atlas displaced terrain"),
            source: wgpu::ShaderSource::Wgsl(shader_source().into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[&uniforms_layout, &source_layout],
            push_constant_ranges: &[],
        });
        let surface = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Atlas elevation surface"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_surface"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_surface"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::LessEqual,
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self {
            format: None,
            surface,
            blit: HashMap::new(),
            uniforms_layout,
            source_layout,
            blit_layout,
            sampler,
            slots: Default::default(),
            generation: GENERATION.load(Ordering::Relaxed),
            keep_globe: crate::gpu_config::settings().atlas_keep_globe,
            #[cfg(test)]
            uploaded_bytes: 0,
        }
    }
    fn make_source(&self, device: &wgpu::Device, raster: Arc<Raster>) -> Source {
        let size = raster.size.map(|v| v as u32);
        let usage = wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING;
        let height = texture(device, size, wgpu::TextureFormat::R32Float, usage);
        let color = texture(device, size, wgpu::TextureFormat::Rgba8Unorm, usage);
        let forest = forest::Forest::empty(device);
        let bind = source_bind(
            device,
            &self.source_layout,
            &self.sampler,
            &height,
            &color,
            &forest.texture,
        );
        Source {
            raster,
            height,
            color,
            bind,
            row: 0,
            forest,
        }
    }
    fn make_target(&self, device: &wgpu::Device, size: [u32; 2]) -> Target {
        let color = texture(
            device,
            size,
            wgpu::TextureFormat::Rgba8Unorm,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        );
        let depth = texture(
            device,
            size,
            wgpu::TextureFormat::Depth32Float,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
        );
        let color_view = color.create_view(&Default::default());
        let depth_view = depth.create_view(&Default::default());
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
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
        Target {
            _color: color,
            color: color_view,
            _depth: depth,
            depth: depth_view,
            bind,
            size,
        }
    }
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        callback: &Callback,
        ppp: f32,
    ) {
        let slot = callback.slot;
        for raster in &callback.data {
            if !self.slots[slot].sources.contains_key(&raster.id) {
                let source = self.make_source(device, raster.clone());
                self.slots[slot].sources.insert(raster.id, source);
            }
        }
        let state = &mut self.slots[slot];
        let mut budget = 8 * 1024 * 1024;
        for raster in &callback.data {
            let source = state.sources.get_mut(&raster.id).unwrap();
            while source.row < raster.size[1] {
                let rows = (512 * 1024 / (raster.size[0] * 8))
                    .max(1)
                    .min(raster.size[1] - source.row);
                let bytes = rows * raster.size[0] * 8;
                if bytes > budget {
                    break;
                }
                let start = source.row * raster.size[0];
                let end = start + rows * raster.size[0];
                let layout = wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(raster.size[0] as u32 * 4),
                    rows_per_image: Some(rows as u32),
                };
                let extent = wgpu::Extent3d {
                    width: raster.size[0] as u32,
                    height: rows as u32,
                    depth_or_array_layers: 1,
                };
                for (texture, bytes) in [
                    (
                        &source.height,
                        bytemuck::cast_slice(&raster.heights[start..end]),
                    ),
                    (&source.color, &raster.colors[start * 4..end * 4]),
                ] {
                    queue.write_texture(
                        wgpu::TexelCopyTextureInfo {
                            texture,
                            mip_level: 0,
                            origin: wgpu::Origin3d {
                                x: 0,
                                y: source.row as u32,
                                z: 0,
                            },
                            aspect: wgpu::TextureAspect::All,
                        },
                        bytes,
                        layout,
                        extent,
                    );
                }
                source.row += rows;
                budget -= bytes;
                #[cfg(test)]
                {
                    self.uploaded_bytes += bytes;
                }
            }
            if source.row < raster.size[1] {
                callback.ctx.request_repaint();
            }
        }
        if let Some(mask) = &callback.forest
            && let Some(source) = state.sources.get_mut(&mask.raster_id)
        {
            if source
                .forest
                .stage(device, queue, mask.clone(), &mut budget)
            {
                source.bind = source_bind(
                    device,
                    &self.source_layout,
                    &self.sampler,
                    &source.height,
                    &source.color,
                    &source.forest.texture,
                );
            }
            if !source.forest.ready(mask) {
                callback.ctx.request_repaint();
            }
        }
        let ready: Vec<_> = callback
            .data
            .iter()
            .filter(|r| state.sources[&r.id].row == r.size[1])
            .map(|r| r.id)
            .collect();
        if slot == 0 || ready.len() == callback.data.len() || state.active.is_empty() {
            state.active = ready;
        }
        let keep: HashSet<_> = callback
            .data
            .iter()
            .map(|r| r.id)
            .chain(state.active.iter().copied())
            .collect();
        state.sources.retain(|id, source| {
            keep.contains(id)
                || (slot == 0 && self.keep_globe && source.row == source.raster.size[1])
        });
        if slot == 0 {
            let uploading = callback.data.len() - state.active.len();
            let changed = callback.ctx.data_mut(|d| {
                let id = egui::Id::new("atlas-globe-uploading");
                let changed = d.get_temp::<usize>(id) != Some(uploading);
                d.insert_temp(id, uploading);
                changed
            });
            if changed {
                callback.ctx.request_repaint();
            }
        }
        let active = state.active.last().copied();
        let old = callback
            .ctx
            .data(|d| d.get_temp::<u64>(egui::Id::new(("atlas-surface-active", callback.slot))));
        callback.ctx.data_mut(|d| {
            let id = egui::Id::new(("atlas-surface-active", callback.slot));
            if let Some(active) = active {
                d.insert_temp(id, active);
            } else {
                d.remove::<u64>(id);
            }
        });
        if active != old {
            callback.ctx.request_repaint();
        }
        let forest_active = callback
            .forest
            .as_ref()
            .filter(|mask| {
                state.active.contains(&mask.raster_id)
                    && state.sources[&mask.raster_id].forest.ready(mask)
            })
            .map(|mask| mask.id);
        let old_forest = callback
            .ctx
            .data(|d| d.get_temp::<u64>(egui::Id::new("atlas-forest-active")));
        callback.ctx.data_mut(|d| {
            let id = egui::Id::new("atlas-forest-active");
            if let Some(mask) = forest_active {
                d.insert_temp(id, mask);
            } else {
                d.remove::<u64>(id);
            }
        });
        if forest_active != old_forest {
            callback.ctx.request_repaint();
        }
        if state.active.is_empty() {
            return;
        }
        let size = [
            (callback.rect.width() * ppp).ceil().max(1.) as u32,
            (callback.rect.height() * ppp).ceil().max(1.) as u32,
        ];
        if state.target.as_ref().is_none_or(|t| t.size != size) {
            let target = self.make_target(device, size);
            self.slots[slot].target = Some(target);
            self.slots[slot].key.clear();
        }
        let state = &mut self.slots[slot];
        use crate::panels::world_map::atlas_globe_tiles::{COLS, ROWS};
        let tiled = slot == 0 && callback.data.iter().any(|r| r.core.is_some());
        let mut coverage = vec![0u8; COLS * ROWS];
        for id in &state.active {
            if let Some(core) = state.sources[id].raster.core {
                let col = ((core[0] + 180.) / 5.).round() as usize;
                let row = ((core[1] + 90.) / 5.).round() as usize;
                if col < COLS && row < ROWS {
                    coverage[row * COLS + col] = 1;
                }
            }
        }
        if state.coverage.is_none() {
            state.coverage = Some(texture(
                device,
                [COLS as u32, ROWS as u32],
                wgpu::TextureFormat::R8Uint,
                wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
            ));
        }
        if state.coverage_bytes != coverage {
            queue.write_texture(
                state.coverage.as_ref().unwrap().as_image_copy(),
                &coverage,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(COLS as u32),
                    rows_per_image: Some(ROWS as u32),
                },
                wgpu::Extent3d {
                    width: COLS as u32,
                    height: ROWS as u32,
                    depth_or_array_layers: 1,
                },
            );
            state.coverage_bytes = coverage;
        }
        let coverage_view = state
            .coverage
            .as_ref()
            .unwrap()
            .create_view(&Default::default());
        let mut draws = Vec::new();
        let mut key = state.coverage_bytes.clone();
        for (index, id) in state.active.iter().enumerate() {
            let raster = &state.sources[id].raster;
            let mut u = callback.uniforms[index.min(callback.uniforms.len() - 1)].reframe(raster);
            if tiled && raster.core.is_none() {
                u.extra[2] = 2.0;
            } else if !tiled && callback.slot == 0 && index == 0 && state.active.len() > 1 {
                u.extra[2] = 1.0;
                u.exclude = state.sources[state.active.last().unwrap()].raster.bounds;
            }
            if slot == 0 && raster.core.is_some() {
                for axis in 0..2 {
                    u.grid[axis] = (u.grid[axis] * ppp)
                        .ceil()
                        .min((raster.size[axis] - 1) as f32);
                }
            }
            let forest_id = callback
                .forest
                .as_ref()
                .filter(|mask| mask.raster_id == *id && state.sources[id].forest.ready(mask))
                .map_or(0, |mask| mask.id);
            u.appearance[2] = u32::from(forest_id != 0) as f32;
            key.extend_from_slice(&forest_id.to_le_bytes());
            key.extend_from_slice(&id.to_le_bytes());
            key.extend_from_slice(bytemuck::bytes_of(&u));
            let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::bytes_of(&u),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &self.uniforms_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&coverage_view),
                    },
                ],
            });
            draws.push((*id, u, buffer, bind));
        }
        if callback.slot == 1 && draws.len() > 1 {
            let lo = draws
                .iter()
                .map(|(_, u, _, _)| u.grid[2] - 0.45 / u.grid[3])
                .fold(f32::INFINITY, f32::min);
            let hi = draws
                .iter()
                .map(|(_, u, _, _)| u.grid[2] + 0.45 / u.grid[3])
                .fold(f32::NEG_INFINITY, f32::max);
            key.clear();
            for (id, u, buffer, _) in &mut draws {
                u.grid[2] = (lo + hi) * 0.5;
                u.grid[3] = 0.9 / (hi - lo).max(1.0);
                // Every local source shares one monotonic depth range.
                *buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: None,
                    contents: bytemuck::bytes_of(u),
                    usage: wgpu::BufferUsages::UNIFORM,
                });
                let forest_id = callback
                    .forest
                    .as_ref()
                    .filter(|mask| mask.raster_id == *id && state.sources[id].forest.ready(mask))
                    .map_or(0, |mask| mask.id);
                key.extend_from_slice(&forest_id.to_le_bytes());
                key.extend_from_slice(&id.to_le_bytes());
                key.extend_from_slice(bytemuck::bytes_of(u));
            }
            for (_, _, buffer, bind) in &mut draws {
                *bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: None,
                    layout: &self.uniforms_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: buffer.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(&coverage_view),
                        },
                    ],
                });
            }
        }
        if key == state.key {
            return;
        }
        state.key = key;
        let target = state.target.as_ref().unwrap();
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Atlas true 3D surface"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target.color,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &target.depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&self.surface);
        for (id, u, _, bind) in &draws {
            pass.set_bind_group(0, bind, &[]);
            pass.set_bind_group(1, &state.sources[id].bind, &[]);
            pass.draw(0..(u.grid[0] as u32 + 1) * 2, 0..u.grid[1] as u32);
        }
    }
    pub fn paint(&self, pass: &mut wgpu::RenderPass<'static>, slot: usize) {
        if let Some(target) = &self.slots[slot].target {
            if let Some(pipeline) = self.format.and_then(|f| self.blit.get(&f)) {
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, &target.bind, &[]);
                pass.draw(0..3, 0..1);
            }
        }
    }
    pub fn set_format(&mut self, device: &wgpu::Device, format: wgpu::TextureFormat) {
        self.format = Some(format);
        if self.blit.contains_key(&format) {
            return;
        }
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Atlas surface composite"),
            source: super::super::gpu_color::shader(include_str!("atlas_surface_blit.wgsl")),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[&self.blit_layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: None,
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_blit"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_blit"),
                compilation_options: super::super::gpu_color::fragment_options(format),
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
        self.blit.insert(format, pipeline);
    }
}

#[cfg(test)]
#[path = "atlas_retention_gpu_tests.rs"]
mod retention_tests;
