//! Unified RTC GPU-instanced vector line renderer.
//!
//! Draws geographic vector batches (global coastlines, bathymetry, global topo,
//! SRTM/3DEP/SLDEM/CTX contour tiles, OSM roads, admin borders, pipelines) into
//! the shared Reversed-Z [`UnifiedSceneTarget`] (on desktop) or directly into
//! an OpenXR stereo eye render pass (in VR).
//!
//! Geometry is stored once per batch in tile-local `f32` metres relative to an
//! `f64` ECEF origin, giving sub-millimetre stability at street scale without
//! ever rebuilding vertex buffers on pan, zoom, tilt, or layer-spread changes.

use super::contour_pass::contour_feather_px;
use super::projection::{MapProjection, RtcCameraUniforms, ecef_to_tile_local, geo_to_ecef};
use super::unified_scene_target::{SCENE_COLOR_FORMAT, SCENE_DEPTH_FORMAT, UnifiedSceneTarget};
use crate::model::GeoPoint;
use eframe::{egui_wgpu, wgpu};
use glam::DVec3;
use std::collections::HashMap;
use std::sync::Arc;
use wgpu::util::DeviceExt;

/// One static RTC polyline segment (40 bytes, `VertexStepMode::Instance`).
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct RtcSegmentInstance {
    /// Endpoint A sea-level tile-local ECEF metres + elevation (m).
    pub a_local_elev: [f32; 4],
    /// Endpoint B sea-level tile-local ECEF metres + elevation (m).
    pub b_local_elev: [f32; 4],
    /// Premultiplied RGBA color.
    pub color: [u8; 4],
    /// Bit 0 = major contour flag.
    pub flags: u32,
}

const _: () = assert!(std::mem::size_of::<RtcSegmentInstance>() == 40);

impl RtcSegmentInstance {
    #[inline]
    pub fn from_geo(
        a: GeoPoint,
        a_elev_m: f32,
        b: GeoPoint,
        b_elev_m: f32,
        tile_origin_ecef: DVec3,
        radius_m: f64,
        color: egui::Color32,
        major: bool,
    ) -> Self {
        let pa = ecef_to_tile_local(
            geo_to_ecef(a.lat as f64, a.lon as f64, 0.0, radius_m),
            tile_origin_ecef,
        );
        let pb = ecef_to_tile_local(
            geo_to_ecef(b.lat as f64, b.lon as f64, 0.0, radius_m),
            tile_origin_ecef,
        );
        Self {
            a_local_elev: [pa.x, pa.y, pa.z, a_elev_m],
            b_local_elev: [pb.x, pb.y, pb.z, b_elev_m],
            color: color.to_array(),
            flags: u32::from(major),
        }
    }
}

/// Byte-for-byte mirror of `LineUniforms` in `unified_lines.wgsl` (208 bytes).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct UnifiedLineUniforms {
    pub cam: RtcCameraUniforms,
    pub stroke: [f32; 4],
    pub style: [f32; 4],
    pub flags: [f32; 4],
    pub major_color: [f32; 4],
    pub minor_color: [f32; 4],
    pub eye_unit: [f32; 4],
}

const _: () = assert!(std::mem::size_of::<UnifiedLineUniforms>() == 208);

const UNIFORM_STRIDE: u64 = 256;
const MAX_BATCHES_PER_FRAME: usize = 512;

#[derive(Clone)]
pub struct RtcLineBatch {
    pub key: u64,
    pub version: u64,
    pub origin_ecef: DVec3,
    pub segments: Arc<Vec<RtcSegmentInstance>>,
    pub stroke_minor_px: f32,
    pub stroke_major_px: f32,
    pub alpha: f32,
    pub extra_radial_m: f32,
    pub depth_bias_ndc: f32,
    pub horizon_cull: bool,
    pub palette_override: Option<([f32; 4], [f32; 4])>,
}

struct CachedGpuBatch {
    version: u64,
    buffers: Vec<(wgpu::Buffer, u32)>,
    last_used_frame: u64,
}

pub struct UnifiedLineResources {
    pipeline: wgpu::RenderPipeline,
    uniform_buf: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    batches: HashMap<u64, CachedGpuBatch>,
    frame_counter: u64,
}

impl UnifiedLineResources {
    pub fn new(device: &wgpu::Device) -> Self {
        Self::new_for_formats(device, SCENE_COLOR_FORMAT, SCENE_DEPTH_FORMAT)
    }

    pub fn new_for_formats(
        device: &wgpu::Device,
        color_format: wgpu::TextureFormat,
        depth_format: wgpu::TextureFormat,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("unified_lines_shader"),
            source: wgpu::ShaderSource::Wgsl(UNIFIED_LINES_WGSL.into()),
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("unified_lines_bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(
                        std::mem::size_of::<UnifiedLineUniforms>() as u64,
                    ),
                },
                count: None,
            }],
        });
        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("unified_lines_uniforms"),
            size: UNIFORM_STRIDE * MAX_BATCHES_PER_FRAME as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("unified_lines_bg"),
            layout: &bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &uniform_buf,
                    offset: 0,
                    size: wgpu::BufferSize::new(std::mem::size_of::<UnifiedLineUniforms>() as u64),
                }),
            }],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("unified_lines_pl"),
            bind_group_layouts: &[&bgl],
            push_constant_ranges: &[],
        });
        let attrs = wgpu::vertex_attr_array![
            0 => Float32x4,
            1 => Float32x4,
            2 => Unorm8x4,
            3 => Uint32,
        ];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("unified_lines_pipeline"),
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<RtcSegmentInstance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &attrs,
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: color_format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: depth_format,
                depth_write_enabled: false,
                depth_compare: wgpu::CompareFunction::GreaterEqual,
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });
        Self {
            pipeline,
            uniform_buf,
            bind_group,
            batches: HashMap::new(),
            frame_counter: 0,
        }
    }

    pub fn ensure_uploaded(&mut self, device: &wgpu::Device, batch: &RtcLineBatch) {
        self.frame_counter = self.frame_counter.wrapping_add(1);
        let needs_upload = self
            .batches
            .get(&batch.key)
            .is_none_or(|c| c.version != batch.version);
        if needs_upload {
            let max_buf = device.limits().max_buffer_size.min(128 << 20) as usize;
            let stride = std::mem::size_of::<RtcSegmentInstance>();
            let per_chunk = (max_buf / stride).max(1);
            let buffers = batch
                .segments
                .chunks(per_chunk)
                .map(|chunk| {
                    let buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("unified_line_instances"),
                        contents: bytemuck::cast_slice(chunk),
                        usage: wgpu::BufferUsages::VERTEX,
                    });
                    (buf, chunk.len() as u32)
                })
                .collect();
            self.batches.insert(
                batch.key,
                CachedGpuBatch {
                    version: batch.version,
                    buffers,
                    last_used_frame: self.frame_counter,
                },
            );
        } else if let Some(entry) = self.batches.get_mut(&batch.key) {
            entry.last_used_frame = self.frame_counter;
        }
        // Evict stale batches if cache grows large.
        if self.batches.len() > 768 {
            let cutoff = self.frame_counter.saturating_sub(240);
            self.batches.retain(|_, v| v.last_used_frame >= cutoff);
        }
    }

    pub fn encode_into_scene(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        color_view: &wgpu::TextureView,
        depth_view: &wgpu::TextureView,
        proj: &MapProjection,
        batches: &[RtcLineBatch],
    ) {
        if batches.is_empty() {
            return;
        }
        let count = batches.len().min(MAX_BATCHES_PER_FRAME);
        let eye_unit = proj.eye_ecef.normalize_or(proj.target_up);
        let tanaka = if crate::theme::tanaka_contours() { 1.0 } else { 0.0 };
        let abyss = if crate::theme::is_abyss() { 1.0 } else { 0.0 };

        for (slot, batch) in batches.iter().take(count).enumerate() {
            if batch.segments.is_empty() {
                continue;
            }
            self.ensure_uploaded(device, batch);
            let (pal_on, maj_col, min_col) = match batch.palette_override {
                Some((maj, min)) => (1.0, maj, min),
                None => (0.0, [0.0; 4], [0.0; 4]),
            };
            let u = UnifiedLineUniforms {
                cam: proj.rtc_uniforms(batch.origin_ecef),
                stroke: [
                    batch.stroke_minor_px * 0.5,
                    batch.stroke_major_px * 0.5,
                    contour_feather_px(batch.stroke_minor_px),
                    contour_feather_px(batch.stroke_major_px),
                ],
                style: [batch.alpha, batch.depth_bias_ndc, tanaka, abyss],
                flags: [
                    pal_on,
                    if batch.horizon_cull { 1.0 } else { 0.0 },
                    batch.extra_radial_m,
                    0.0,
                ],
                major_color: maj_col,
                minor_color: min_col,
                eye_unit: [eye_unit.x as f32, eye_unit.y as f32, eye_unit.z as f32, 0.0],
            };
            queue.write_buffer(
                &self.uniform_buf,
                slot as u64 * UNIFORM_STRIDE,
                bytemuck::bytes_of(&u),
            );
        }

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("unified_lines_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&self.pipeline);
        for (slot, batch) in batches.iter().take(count).enumerate() {
            let Some(cached) = self.batches.get(&batch.key) else {
                continue;
            };
            let offset = (slot as u32) * (UNIFORM_STRIDE as u32);
            pass.set_bind_group(0, &self.bind_group, &[offset]);
            for (buf, instance_count) in &cached.buffers {
                pass.set_vertex_buffer(0, buf.slice(..));
                pass.draw(0..4, 0..*instance_count);
            }
        }
    }
}

pub struct UnifiedLineCallback {
    pub proj: MapProjection,
    pub batches: Vec<RtcLineBatch>,
}

impl UnifiedLineCallback {
    pub fn into_paint_callback(self) -> egui::PaintCallback {
        let rect = self.proj.viewport;
        egui_wgpu::Callback::new_paint_callback(rect, self)
    }
}

impl egui_wgpu::CallbackTrait for UnifiedLineCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if self.batches.is_empty() {
            return Vec::new();
        }
        let Some(scene) = resources.get::<UnifiedSceneTarget>() else {
            return Vec::new();
        };
        let Some((color_view, depth_view, _)) = scene.views() else {
            return Vec::new();
        };
        // Clone TextureViews (Arc-backed in wgpu) so we can borrow UnifiedLineResources mutably.
        let color_view = color_view.clone();
        let depth_view = depth_view.clone();
        if let Some(lines) = resources.get_mut::<UnifiedLineResources>() {
            lines.encode_into_scene(
                device,
                queue,
                encoder,
                &color_view,
                &depth_view,
                &self.proj,
                &self.batches,
            );
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

const UNIFIED_LINES_WGSL: &str = concat!(
    include_str!("unified_camera.wgsl"),
    "\n",
    include_str!("unified_lines.wgsl")
);
