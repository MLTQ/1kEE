//! Cached depth-only visibility, isolated from egui's depth-free UI render pass.
use super::*;
const FRAME_SEGMENTS: u32 = 65_536;
const MAX_TEXTURE_EDGE: u32 = 2048;

pub(super) fn texture_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("contour occlusion texture layout"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Depth,
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        }],
    })
}
struct Target {
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
    bind: wgpu::BindGroup,
}
fn target(device: &wgpu::Device, size: [u32; 2]) -> Target {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("contour occlusion depth"),
        size: wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth32Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("contour occlusion depth binding"),
        layout: &texture_layout(device),
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::TextureView(&view),
        }],
    });
    Target {
        _texture: texture,
        view,
        bind,
    }
}

pub(super) struct Resources {
    pipeline: wgpu::RenderPipeline,
    dummy: Target,
    target: Option<Target>,
    size: [u32; 2],
    pub(super) key: Vec<u8>,
    last_used: u64,
    #[cfg(test)]
    pub renders: usize,
}
impl Resources {
    pub fn new(
        device: &wgpu::Device,
        uniforms: &wgpu::BindGroupLayout,
        shader: &wgpu::ShaderModule,
    ) -> Self {
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("contour curtain layout"),
            bind_group_layouts: &[uniforms],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("contour curtains depth only"), layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: shader, entry_point: Some("vs_curtain"), compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<LocalSegmentInstance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Unorm8x4, 3 => Uint32],
                }],
            },
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleStrip, ..Default::default() },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float, depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less, stencil: Default::default(), bias: Default::default(),
            }),
            fragment: None, multisample: Default::default(), multiview: None, cache: None,
        });
        Self {
            pipeline,
            dummy: target(device, [1, 1]),
            target: None,
            size: [0, 0],
            key: vec![],
            last_used: 0,
            #[cfg(test)]
            renders: 0,
        }
    }
    pub fn bind_group(&self) -> &wgpu::BindGroup {
        &self.target.as_ref().unwrap_or(&self.dummy).bind
    }
    pub fn release_if_unused(&mut self, frame: u64) {
        if self.last_used != frame {
            self.target = None;
            self.key.clear();
        }
    }
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        uniforms_bind: &wgpu::BindGroup,
        u: &LocalContourUniforms,
        batches: &[LocalTileBatch],
        tiles: &HashMap<(LocalBatchId, u64), TileGpu>,
        frame: u64,
    ) {
        if u.occlusion_enabled == 0.0 {
            self.target = None;
            self.key.clear();
            return;
        }
        self.last_used = frame;
        let downscale =
            (u.viewport_size_x.max(u.viewport_size_y) / MAX_TEXTURE_EDGE as f32).max(1.0);
        let size = [
            (u.viewport_size_x / downscale).ceil().max(1.0) as u32,
            (u.viewport_size_y / downscale).ceil().max(1.0) as u32,
        ];
        let mut key = bytemuck::bytes_of(u).to_vec();
        let mut drawable: Vec<_> = batches
            .iter()
            .filter_map(|b| {
                tiles
                    .get(&(b.id, b.version))
                    .filter(|tile| !tile.occlusion.is_empty())
                    .map(|tile| (b, tile))
            })
            .collect();
        // Near tiles first improve early depth rejection. The tie breakers and
        // fair per-tile quota keep the hard frame budget deterministic.
        drawable.sort_by(|(a, _), (b, _)| {
            center_depth(u, a)
                .total_cmp(&center_depth(u, b))
                .then_with(|| format!("{:?}", a.id).cmp(&format!("{:?}", b.id)))
        });
        for (batch, _) in &drawable {
            key.extend_from_slice(format!("{:?}", batch.id).as_bytes());
            key.extend_from_slice(&batch.version.to_le_bytes());
        }
        if self.target.is_some() && self.size == size && self.key == key {
            return;
        }
        if self.target.is_none() || self.size != size {
            self.target = Some(target(device, size));
            self.size = size;
        }
        self.key = key;
        #[cfg(test)]
        {
            self.renders += 1;
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("contour occlusion prepass"),
            color_attachments: &[],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.target.as_ref().unwrap().view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, uniforms_bind, &[0]);
        let quota = FRAME_SEGMENTS / drawable.len().max(1) as u32;
        for (_, tile) in drawable {
            let mut remaining = quota;
            for chunk in &tile.occlusion {
                let count = chunk.count.min(remaining);
                if count == 0 {
                    break;
                }
                pass.set_vertex_buffer(0, chunk.buffer.slice(..));
                pass.draw(0..4, 0..count);
                remaining -= count;
            }
        }
    }
}
fn ground_depth(u: &LocalContourUniforms, lon: f32, lat: f32) -> f32 {
    (lon - u.focus_lon) * u.x_factor * u.yaw_sin + (lat - u.focus_lat) * u.y_factor * u.yaw_cos
}
fn center_depth(u: &LocalContourUniforms, b: &LocalTileBatch) -> f32 {
    let r = b.occlusion.bounds;
    ground_depth(u, (r[0] + r[2]) * 0.5, (r[1] + r[3]) * 0.5)
}
/// Ground-distance depth is monotonic along a viewing ray for this affine
/// projector: increasing ground distance while lowering height moves behind.
/// Unlike the old clamped pitch depth, it cannot reverse with layer spread.
pub(super) fn configure(u: &mut LocalContourUniforms, batches: &[LocalTileBatch]) {
    let ground = u.pitch_cos * u.ground_pitch_scale - u.pitch_sin * u.ground_depth_scale;
    let vertical = u.pitch_sin * u.elevation_pitch_scale + u.pitch_cos * u.elevation_depth_scale;
    if u.occlusion_enabled == 0.0 || ground <= 0.0 || vertical <= 0.0 || u.z_factor <= 0.0 {
        u.occlusion_enabled = 0.0;
        return;
    }
    let mut lo = f32::INFINITY;
    let mut hi = f32::NEG_INFINITY;
    let mut error = 0.0f32;
    for b in batches.iter().filter(|b| !b.occlusion.segments.is_empty()) {
        let r = b.occlusion.bounds;
        for lon in [r[0], r[2]] {
            for lat in [r[1], r[3]] {
                let d = ground_depth(u, lon, lat);
                lo = lo.min(d);
                hi = hi.max(d);
            }
        }
        error = error.max(b.occlusion.error_deg);
    }
    if !lo.is_finite() || !hi.is_finite() {
        u.occlusion_enabled = 0.0;
        return;
    }
    let margin = (hi - lo).max(1.0);
    u.depth_center = (hi + lo) * 0.5;
    u.depth_scale = 0.8 / (hi - lo + 2.0 * margin);
    let distance_error = error * (u.x_factor.abs() + u.y_factor.abs());
    u.depth_bias = (distance_error * 2.0 + 1e-5) * u.depth_scale;
    u.screen_guard = 2.0
        * (u.viewport_size_x.max(u.viewport_size_y) / MAX_TEXTURE_EDGE as f32).max(1.0)
        + distance_error * ground.abs() * u.pixels_per_point;
}
