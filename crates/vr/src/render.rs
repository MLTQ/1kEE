use crate::scene::{Globe, MAX_VERTICES, Marker, pose, projection, surface};
use anyhow::Result;
use bytemuck::{Pod, Zeroable};
use openxr as xr;
use std::sync::mpsc::{self, Receiver};
use wgpu::util::DeviceExt;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniforms {
    vp: [[f32; 4]; 4],
    inv_vp: [[f32; 4]; 4],
    brief_vp: [[f32; 4]; 4],
    eye: [f32; 4],
    viewport: [f32; 4],
    marker: [f32; 4],
    cursor: [f32; 4],
    palette: [[f32; 4]; 5],
    radius: f32,
    show_graticule: u32,
    active_body: u32,
    pad: u32,
}

pub struct Renderer {
    globe: wgpu::RenderPipeline,
    lines: wgpu::RenderPipeline,
    brief: wgpu::RenderPipeline,
    eyes: Vec<(wgpu::Buffer, wgpu::BindGroup)>,
    vertices: wgpu::Buffer,
    published: u32,
    uploaded: usize,
}
impl Renderer {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        marker: &Marker,
    ) -> Result<Self> {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("1kEE stereo globe"),
            source: wgpu::ShaderSource::Wgsl(
                concat!(
                    include_str!("../../desktop/src/panels/world_map/globe_surface.wgsl"),
                    "\n",
                    include_str!("globe.wgsl")
                )
                .into(),
            ),
        });
        let relief = image::load_from_memory(include_bytes!("../assets/earth.png"))?.into_luma8();
        let size = wgpu::Extent3d {
            width: relief.width(),
            height: relief.height(),
            depth_or_array_layers: 1,
        };
        let map = device.create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some("Natural Earth shaded relief"),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            relief.as_raw(),
        );
        let view = map.create_view(&Default::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let brief_view = crate::brief::texture(device, queue, marker);
        let entries = [
            wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
        ];
        let bindings = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("stereo bindings"),
            entries: &entries
                .into_iter()
                .enumerate()
                .map(|(i, ty)| wgpu::BindGroupLayoutEntry {
                    binding: i as _,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty,
                    count: None,
                })
                .collect::<Vec<_>>(),
        });
        let eyes = (0..2)
            .map(|_| {
                let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("eye pose"),
                    size: std::mem::size_of::<Uniforms>() as _,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::UNIFORM,
                    mapped_at_creation: false,
                });
                let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("eye"),
                    layout: &bindings,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: buffer.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(&view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::Sampler(&sampler),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: wgpu::BindingResource::TextureView(&brief_view),
                        },
                    ],
                });
                (buffer, group)
            })
            .collect();
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("globe"),
            bind_group_layouts: &[&bindings],
            push_constant_ranges: &[],
        });
        let pipeline = |vertex, fragment, topology, buffers: &[wgpu::VertexBufferLayout]| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(vertex),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(vertex),
                    compilation_options: Default::default(),
                    buffers,
                },
                primitive: wgpu::PrimitiveState {
                    topology,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fragment),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview: None,
                cache: None,
            })
        };
        let globe = pipeline(
            "vs_main",
            "fs_main",
            wgpu::PrimitiveTopology::TriangleList,
            &[],
        );
        let lines = pipeline(
            "vs_line",
            "fs_line",
            wgpu::PrimitiveTopology::LineList,
            &[wgpu::VertexBufferLayout {
                array_stride: 12,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![0 => Float32x3],
            }],
        );
        let brief = pipeline(
            "vs_brief",
            "fs_brief",
            wgpu::PrimitiveTopology::TriangleStrip,
            &[],
        );
        let vertices = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("resident contour tile"),
            size: (MAX_VERTICES * 12) as _,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Ok(Self {
            globe,
            lines,
            brief,
            eyes,
            vertices,
            published: 0,
            uploaded: 0,
        })
    }

    pub fn upload_chunk(&mut self, queue: &wgpu::Queue, data: &[[f32; 3]]) -> bool {
        // The buffer already exists. Publish to both eyes only after the last chunk.
        let end = (self.uploaded + 4096).min(data.len());
        queue.write_buffer(
            &self.vertices,
            (self.uploaded * 12) as _,
            bytemuck::cast_slice(&data[self.uploaded..end]),
        );
        self.uploaded = end;
        if end == data.len() {
            self.published = end as u32;
            true
        } else {
            false
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        index: usize,
        target: &wgpu::TextureView,
        size: [u32; 2],
        view: xr::View,
        globe: &Globe,
        marker: &Marker,
        selected: bool,
        cursor: Option<glam::Vec3>,
        timestamps: Option<wgpu::RenderPassTimestampWrites>,
    ) {
        let modelview = pose(view.pose).inverse() * globe.transform();
        let vp = projection(view.fov) * modelview;
        let eye = modelview.inverse().w_axis;
        let data = Uniforms {
            vp: vp.to_cols_array_2d(),
            inv_vp: vp.inverse().to_cols_array_2d(),
            brief_vp: (projection(view.fov) * pose(view.pose).inverse() * globe.brief_transform())
                .to_cols_array_2d(),
            cursor: cursor.map_or([0.; 4], |v| v.extend(1.).to_array()),
            eye: eye.to_array(),
            viewport: [size[0] as _, size[1] as _, globe.scale, 0.],
            marker: surface(marker.lat, marker.lon)
                .extend(u8::from(selected) as f32)
                .to_array(),
            palette: [
                [0.05, 0.12, 0.20, 1.],
                [0.16, 0.38, 0.30, 1.],
                [0.78, 0.76, 0.66, 1.],
                [0.6, 0.75, 0.8, 1.],
                [1., 0.62, 0.22, 1.],
            ],
            radius: 300.,
            show_graticule: 1,
            active_body: 0,
            pad: 0,
        };
        let (buf, bind) = &self.eyes[index];
        queue.write_buffer(buf, 0, bytemuck::bytes_of(&data));
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("globe eye"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: timestamps,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&self.globe);
        pass.set_bind_group(0, bind, &[]);
        pass.draw(0..3, 0..1);
        if self.published > 0 {
            pass.set_pipeline(&self.lines);
            pass.set_vertex_buffer(0, self.vertices.slice(..));
            pass.draw(0..self.published, 0..1);
        }
        if selected {
            pass.set_pipeline(&self.brief);
            pass.draw(0..4, 0..1);
        }
    }
}

// Sample the stereo GPU interval without a blocking poll in the headset loop.
pub struct GpuTimer {
    query: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    readback: wgpu::Buffer,
    pending: Option<Receiver<Result<(), wgpu::BufferAsyncError>>>,
}
impl GpuTimer {
    pub fn new(device: &wgpu::Device) -> Option<Self> {
        if !device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
            return None;
        }
        let query = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("stereo GPU time"),
            ty: wgpu::QueryType::Timestamp,
            count: 2,
        });
        let buffer = |usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("GPU timings"),
                size: 16,
                usage,
                mapped_at_creation: false,
            })
        };
        Some(Self {
            query,
            resolve: buffer(wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC),
            readback: buffer(wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ),
            pending: None,
        })
    }
    pub fn writes(&self, eye: usize) -> Option<wgpu::RenderPassTimestampWrites<'_>> {
        self.pending
            .is_none()
            .then_some(wgpu::RenderPassTimestampWrites {
                query_set: &self.query,
                beginning_of_pass_write_index: (eye == 0).then_some(0),
                end_of_pass_write_index: (eye == 1).then_some(1),
            })
    }
    pub fn resolve(&self, encoder: &mut wgpu::CommandEncoder) {
        if self.pending.is_none() {
            encoder.resolve_query_set(&self.query, 0..2, &self.resolve, 0);
            encoder.copy_buffer_to_buffer(&self.resolve, 0, &self.readback, 0, 16);
        }
    }
    pub fn submitted(&mut self) {
        if self.pending.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |v| {
                let _ = tx.send(v);
            });
        self.pending = Some(rx);
    }
    pub fn sample(&mut self, period: f32) -> Option<f64> {
        let result = self.pending.as_ref()?.try_recv().ok()?;
        self.pending = None;
        result.ok()?;
        let range = self.readback.slice(..).get_mapped_range();
        let ticks: &[u64] = bytemuck::cast_slice(&range);
        let ms = ticks[1].saturating_sub(ticks[0]) as f64 * period as f64 / 1e6;
        drop(range);
        self.readback.unmap();
        Some(ms)
    }
}

#[cfg(test)]
#[path = "stereo_test.rs"]
mod stereo_test;
