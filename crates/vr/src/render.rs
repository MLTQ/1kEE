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
    scene_vp: [[f32; 4]; 4],
    stage_vp: [[f32; 4]; 4],
    world_eye: [f32; 4],
    laser_start: [f32; 4],
    laser_end: [f32; 4],
    wrist_vp: [[f32; 4]; 4],
    table_inverse: [[f32; 4]; 4],
    table_eye: [f32; 4],
    source_projection: [f32; 4],
    source_rotation: [f32; 4],
    layers: [f32; 4],
    wrist_cursor: [f32; 4],
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
    pub cartography: crate::cartography::Cartography,
    device: wgpu::Device,
    depths: [Option<([u32; 2], wgpu::TextureView)>; 2],
    spire: wgpu::RenderPipeline,
    laser: wgpu::RenderPipeline,
    wrist_laser: wgpu::RenderPipeline,
    wrist: wgpu::RenderPipeline,
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
        let cartography = crate::cartography::Cartography::new(device);
        let array = |filterable| wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable },
            view_dimension: wgpu::TextureViewDimension::D2Array,
            multisampled: false,
        };
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
            array(true),
            array(true),
            array(false),
            array(true),
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
                        wgpu::BindGroupEntry {
                            binding: 4,
                            resource: wgpu::BindingResource::TextureView(cartography.map_view()),
                        },
                        wgpu::BindGroupEntry {
                            binding: 5,
                            resource: wgpu::BindingResource::TextureView(cartography.wrist_view()),
                        },
                        wgpu::BindGroupEntry {
                            binding: 6,
                            resource: wgpu::BindingResource::TextureView(&cartography.height_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 7,
                            resource: wgpu::BindingResource::TextureView(cartography.room_view()),
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
        let pipeline =
            |vertex, fragment, topology, buffers: &[wgpu::VertexBufferLayout], foreground: bool| {
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
                    depth_stencil: Some(wgpu::DepthStencilState {
                        format: wgpu::TextureFormat::Depth32Float,
                        depth_write_enabled: !foreground,
                        depth_compare: if foreground {
                            wgpu::CompareFunction::Always
                        } else {
                            wgpu::CompareFunction::LessEqual
                        },
                        stencil: Default::default(),
                        bias: Default::default(),
                    }),
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
            false,
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
            false,
        );
        let brief = pipeline(
            "vs_brief",
            "fs_brief",
            wgpu::PrimitiveTopology::TriangleStrip,
            &[],
            false,
        );
        let wrist = pipeline(
            "vs_wrist",
            "fs_wrist",
            wgpu::PrimitiveTopology::TriangleStrip,
            &[],
            true,
        );
        let spire = pipeline(
            "vs_spire",
            "fs_spire",
            wgpu::PrimitiveTopology::TriangleList,
            &[wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<crate::spires::Instance>() as u64,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32, 2 => Float32x3, 3 => Unorm8x4],
            }],
            false,
        );
        let laser = pipeline(
            "vs_laser",
            "fs_laser",
            wgpu::PrimitiveTopology::TriangleStrip,
            &[],
            false,
        );
        let wrist_laser = pipeline(
            "vs_laser",
            "fs_laser",
            wgpu::PrimitiveTopology::TriangleStrip,
            &[],
            true,
        );
        let vertices = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("resident contour tile"),
            size: (MAX_VERTICES * 12) as _,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Ok(Self {
            device: device.clone(),
            depths: [None, None],
            laser,
            wrist_laser,
            spire,
            wrist,
            cartography,
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
        &mut self,
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
        let world_eye = pose(view.pose);
        let camera = projection(view.fov) * world_eye.inverse();
        let table = globe.table_transform();
        let native = &self.cartography;
        let f = native.frame.as_ref();
        let [yaw, pitch] = f.map_or([0.; 2], |f| f.angles);
        let (start, end) = native.laser.unwrap_or_default();
        let data = Uniforms {
            scene_vp: if f.is_some_and(|f| f.local) {
                camera * table
            } else {
                vp
            }
            .to_cols_array_2d(),
            stage_vp: camera.to_cols_array_2d(),
            world_eye: world_eye.w_axis.to_array(),
            laser_start: start.extend(1.).to_array(),
            laser_end: end.extend(f32::from(native.aiming_at_target)).to_array(),
            wrist_vp: (camera * native.wrist_pose.unwrap_or(glam::Mat4::IDENTITY))
                .to_cols_array_2d(),
            table_inverse: (camera * table).inverse().to_cols_array_2d(),
            table_eye: (table.inverse() * world_eye.w_axis).to_array(),
            source_projection: f.map_or([0.; 4], |f| f.projection),
            source_rotation: [yaw.cos(), yaw.sin(), pitch.cos(), pitch.sin()],
            layers: native.layers(),
            wrist_cursor: native
                .pointer
                .map_or([-1., -1., 0., native.room_layer()], |p| {
                    [p.x, p.y, 1., native.room_layer()]
                }),
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
        let depth = &mut self.depths[index];
        if depth.as_ref().is_none_or(|(old, _)| *old != size) {
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("eye scene depth"),
                size: wgpu::Extent3d {
                    width: size[0],
                    height: size[1],
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Depth32Float,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            *depth = Some((size, texture.create_view(&Default::default())));
        }
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
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &depth.as_ref().unwrap().1,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.),
                    store: wgpu::StoreOp::Discard,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: timestamps,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&self.globe);
        pass.set_bind_group(0, bind, &[]);
        pass.draw(0..3, 0..1);
        if self.published > 0 && self.cartography.frame.is_none() {
            pass.set_pipeline(&self.lines);
            pass.set_vertex_buffer(0, self.vertices.slice(..));
            pass.draw(0..self.published, 0..1);
        }
        if !native.spires.instances.is_empty() {
            pass.set_pipeline(&self.spire);
            pass.set_vertex_buffer(0, native.spires.buffer.slice(..));
            pass.draw(0..24, 0..native.spires.instances.len() as u32);
        }
        if (selected && native.frame.is_none())
            || (native.show_room && native.room_ready && native.frame.is_some())
        {
            pass.set_pipeline(&self.brief);
            pass.draw(0..4, 0..1);
        }
        let has_beam = native.laser.is_some() && start.distance_squared(end) > 1e-8;
        if has_beam && native.pointer.is_none() {
            pass.set_pipeline(&self.laser);
            pass.draw(0..4, 0..1);
        }
        if native.wrist_ready && native.wrist_pose.is_some() {
            pass.set_pipeline(&self.wrist);
            pass.draw(0..4, 0..1);
        }
        if has_beam && native.pointer.is_some() {
            pass.set_pipeline(&self.wrist_laser);
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
pub(crate) mod stereo_test;
