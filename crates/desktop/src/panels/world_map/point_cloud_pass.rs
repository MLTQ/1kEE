//! Unified RTC GPU point-cloud renderer for the fused LiDAR sidebar layer.
//!
//! Renders point batches directly into [`UnifiedSceneTarget`]'s Reversed-Z
//! `Depth32Float` buffer (desktop) or stereo OpenXR eye targets (VR) with
//! default point size `1.0` px.

use super::projection::{MapProjection, RtcCameraUniforms};
use super::unified_scene_target::{SCENE_COLOR_FORMAT, SCENE_DEPTH_FORMAT, UnifiedSceneTarget};
use crate::target_mode::heatmap;
use eframe::{egui_wgpu, wgpu};
use glam::DVec3;
use std::collections::HashMap;
use std::sync::Arc;
use wgpu::util::DeviceExt;

/// One RTC LiDAR point (24 bytes, `VertexStepMode::Instance`).
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct RtcPointVertex {
    /// xyz = sea-level tile-local ECEF metres relative to batch `origin_ecef`; w = elevation (m).
    pub local_elev: [f32; 4],
    /// Display RGB (or classification fallback).
    pub color: [u8; 4],
    /// Bits 0..7 = ASPRS class, 8..23 = intensity, 24..27 = return, 31 = has_rgb.
    pub packed: u32,
}

const _: () = assert!(std::mem::size_of::<RtcPointVertex>() == 24);

#[derive(Clone)]
pub struct RtcPointBatch {
    pub id: u64,
    pub origin_ecef: DVec3,
    pub points: Arc<Vec<RtcPointVertex>>,
}

/// Byte-for-byte mirror of `PointCloudUniforms` in `point_cloud.wgsl` (288 bytes).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct PointCloudUniforms {
    cam: RtcCameraUniforms,
    params: [f32; 4],
    flags: [u32; 4],
    heat_colors: [[f32; 4]; heatmap::COLORS.len()],
    extra: [f32; 4],
    extra2: [f32; 4],
}

const _: () = assert!(std::mem::size_of::<PointCloudUniforms>() == 288);

const UNIFORM_STRIDE: u64 = 512;
const MAX_BATCHES_PER_FRAME: usize = 256;

struct GpuPointBatch {
    buffers: Vec<(wgpu::Buffer, u32)>,
    last_used: u64,
}

pub struct PointCloudResources {
    pipeline: wgpu::RenderPipeline,
    uniform_buf: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    batches: HashMap<u64, GpuPointBatch>,
    tick: u64,
}

impl PointCloudResources {
    pub fn new(device: &wgpu::Device) -> Self {
        Self::new_for_formats(device, SCENE_COLOR_FORMAT, SCENE_DEPTH_FORMAT)
    }

    pub fn new_for_formats(
        device: &wgpu::Device,
        color_format: wgpu::TextureFormat,
        depth_format: wgpu::TextureFormat,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("unified_point_cloud_shader"),
            source: wgpu::ShaderSource::Wgsl(POINT_CLOUD_WGSL.into()),
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("unified_point_cloud_bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(
                        std::mem::size_of::<PointCloudUniforms>() as u64,
                    ),
                },
                count: None,
            }],
        });
        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("unified_point_cloud_uniforms"),
            size: UNIFORM_STRIDE * MAX_BATCHES_PER_FRAME as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("unified_point_cloud_bg"),
            layout: &bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &uniform_buf,
                    offset: 0,
                    size: wgpu::BufferSize::new(std::mem::size_of::<PointCloudUniforms>() as u64),
                }),
            }],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("unified_point_cloud_pl"),
            bind_group_layouts: &[&bgl],
            push_constant_ranges: &[],
        });
        let attrs = wgpu::vertex_attr_array![
            0 => Float32x4,
            1 => Unorm8x4,
            2 => Uint32,
        ];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("unified_point_cloud_pipeline"),
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<RtcPointVertex>() as u64,
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
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: depth_format,
                depth_write_enabled: true,
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
            tick: 0,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn encode_into_scene(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        color_view: &wgpu::TextureView,
        depth_view: &wgpu::TextureView,
        proj: &MapProjection,
        batches: &[RtcPointBatch],
        point_size_px: f32,
        class_mask: u32,
        color_mode: u32,
        elev_range: [f32; 2],
    ) {
        if batches.is_empty() {
            return;
        }
        self.tick = self.tick.wrapping_add(1);
        let count = batches.len().min(MAX_BATCHES_PER_FRAME);
        let max_buf = device.limits().max_buffer_size.min(128 << 20) as usize;
        let stride = std::mem::size_of::<RtcPointVertex>();
        let per_chunk = (max_buf / stride).max(1);

        let heat_colors = heatmap::COLORS.map(|c| {
            [
                c[0] as f32 / 255.0,
                c[1] as f32 / 255.0,
                c[2] as f32 / 255.0,
                1.0,
            ]
        });

        for (slot, batch) in batches.iter().take(count).enumerate() {
            if batch.points.is_empty() {
                continue;
            }
            if let Some(existing) = self.batches.get_mut(&batch.id) {
                existing.last_used = self.tick;
            } else {
                let buffers = batch
                    .points
                    .chunks(per_chunk)
                    .map(|chunk| {
                        let buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("unified_point_cloud_batch"),
                            contents: bytemuck::cast_slice(chunk),
                            usage: wgpu::BufferUsages::VERTEX,
                        });
                        (buf, chunk.len() as u32)
                    })
                    .collect();
                self.batches.insert(
                    batch.id,
                    GpuPointBatch {
                        buffers,
                        last_used: self.tick,
                    },
                );
            }

            let u = PointCloudUniforms {
                cam: proj.rtc_uniforms(batch.origin_ecef),
                params: [
                    point_size_px.clamp(1.0, 8.0),
                    elev_range[0],
                    elev_range[1],
                    1.0,
                ],
                flags: [class_mask, color_mode, 0, 0],
                heat_colors,
                extra: [0.0; 4],
                extra2: [0.0; 4],
            };
            queue.write_buffer(
                &self.uniform_buf,
                slot as u64 * UNIFORM_STRIDE,
                bytemuck::bytes_of(&u),
            );
        }

        // Retain active batches and prune evicted ones.
        if self.batches.len() > MAX_BATCHES_PER_FRAME * 2 {
            let cutoff = self.tick.saturating_sub(180);
            self.batches.retain(|_, v| v.last_used >= cutoff);
        }

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("unified_point_cloud_pass"),
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
            let Some(gpu) = self.batches.get(&batch.id) else {
                continue;
            };
            pass.set_bind_group(0, &self.bind_group, &[(slot as u32) * (UNIFORM_STRIDE as u32)]);
            for (buf, instance_count) in &gpu.buffers {
                pass.set_vertex_buffer(0, buf.slice(..));
                pass.draw(0..3, 0..*instance_count);
            }
        }
    }
}

pub struct PointCloudCallback {
    pub proj: MapProjection,
    pub batches: Vec<RtcPointBatch>,
    pub point_size_px: f32,
    pub class_mask: u32,
    pub color_mode: u32,
    pub elev_range: [f32; 2],
}

impl PointCloudCallback {
    pub fn into_paint_callback(self) -> egui::PaintCallback {
        let rect = self.proj.viewport;
        egui_wgpu::Callback::new_paint_callback(rect, self)
    }
}

impl egui_wgpu::CallbackTrait for PointCloudCallback {
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
        let color_view = color_view.clone();
        let depth_view = depth_view.clone();
        if let Some(pc) = resources.get_mut::<PointCloudResources>() {
            pc.encode_into_scene(
                device,
                queue,
                encoder,
                &color_view,
                &depth_view,
                &self.proj,
                &self.batches,
                self.point_size_px,
                self.class_mask,
                self.color_mode,
                self.elev_range,
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

const POINT_CLOUD_WGSL: &str = concat!(
    include_str!("unified_camera.wgsl"),
    "\n",
    include_str!("point_cloud.wgsl")
);
