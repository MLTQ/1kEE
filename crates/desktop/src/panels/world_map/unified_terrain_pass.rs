//! Unified RTC displaced 3D Atlas terrain renderer.
//!
//! Draws overview, 5° GEBCO tiles, SRTM, and fine DEM rasters through a single
//! Reversed-Z pipeline directly into [`UnifiedSceneTarget`] (desktop) or stereo
//! OpenXR eye targets (VR), supporting both Perspective and Orthographic modes.

use super::atlas_forest;
use super::atlas_surface::Raster;
use super::projection::{MapProjection, RtcCameraUniforms, geo_to_ecef, local_enu_basis};
use super::unified_scene_target::{SCENE_COLOR_FORMAT, SCENE_DEPTH_FORMAT, UnifiedSceneTarget};
use eframe::{egui_wgpu, wgpu};
use std::collections::HashMap;
use std::sync::Arc;

/// Byte-for-byte mirror of `Uniforms` in `unified_terrain.wgsl` (320 bytes).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct UnifiedTerrainUniforms {
    pub cam: RtcCameraUniforms, // 112 bytes
    pub region: [f32; 4],
    pub core: [f32; 4],
    pub tile_east: [f32; 4],
    pub tile_north: [f32; 4],
    pub tile_up: [f32; 4],
    pub grid: [f32; 4],
    pub style: [f32; 4],
    pub appearance: [f32; 4],
    pub light: [f32; 4],
    pub major: [f32; 4],
    pub minor: [f32; 4],
    pub exclude: [f32; 4],
    pub extra: [f32; 4],
}

const _: () = assert!(std::mem::size_of::<UnifiedTerrainUniforms>() == 320);

const UNIFORM_STRIDE: u64 = 512; // 256-aligned stride >= 320
const MAX_TILES_PER_FRAME: usize = 256;

struct GpuTile {
    _height: wgpu::Texture,
    _color: wgpu::Texture,
    _forest: wgpu::Texture,
    forest_id: u64,
    bind_group: wgpu::BindGroup,
    last_used: u64,
    bytes: usize,
}

pub struct UnifiedTerrainResources {
    pipeline: wgpu::RenderPipeline,
    source_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniform_buf: wgpu::Buffer,
    coverage_tex: wgpu::Texture,
    uniform_bind: wgpu::BindGroup,
    tiles: HashMap<u64, GpuTile>,
    total_bytes: usize,
    budget_bytes: usize,
    tick: u64,
}

impl UnifiedTerrainResources {
    pub fn new(device: &wgpu::Device) -> Self {
        Self::new_for_formats(device, SCENE_COLOR_FORMAT, SCENE_DEPTH_FORMAT)
    }

    pub fn new_for_formats(
        device: &wgpu::Device,
        color_format: wgpu::TextureFormat,
        depth_format: wgpu::TextureFormat,
    ) -> Self {
        let uniforms_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("unified_terrain_uniforms_bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: wgpu::BufferSize::new(
                            std::mem::size_of::<UnifiedTerrainUniforms>() as u64,
                        ),
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

        let tex_entry = |binding, filterable| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let source_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("unified_terrain_source_bgl"),
            entries: &[
                tex_entry(0, false),
                tex_entry(1, true),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                tex_entry(3, true),
            ],
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("unified_terrain_shader"),
            source: wgpu::ShaderSource::Wgsl(UNIFIED_TERRAIN_WGSL.into()),
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("unified_terrain_pl"),
            bind_group_layouts: &[&uniforms_layout, &source_layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("unified_terrain_pipeline"),
            layout: Some(&pl),
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
                    format: color_format,
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

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("unified_terrain_sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("unified_terrain_uniforms"),
            size: UNIFORM_STRIDE * MAX_TILES_PER_FRAME as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let coverage_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("unified_terrain_coverage"),
            size: wgpu::Extent3d {
                width: 72,
                height: 36,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Uint,
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let coverage_view = coverage_tex.create_view(&Default::default());
        let uniform_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("unified_terrain_uniform_bg"),
            layout: &uniforms_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &uniform_buf,
                        offset: 0,
                        size: wgpu::BufferSize::new(
                            std::mem::size_of::<UnifiedTerrainUniforms>() as u64,
                        ),
                    }),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&coverage_view),
                },
            ],
        });

        Self {
            pipeline,
            source_layout,
            sampler,
            uniform_buf,
            coverage_tex,
            uniform_bind,
            tiles: HashMap::new(),
            total_bytes: 0,
            budget_bytes: crate::gpu_config::settings().atlas_gpu_bytes.max(256 << 20),
            tick: 0,
        }
    }

    fn ensure_tile(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        raster: &Arc<Raster>,
        forest: Option<&Arc<atlas_forest::Mask>>,
    ) {
        self.tick = self.tick.wrapping_add(1);
        let want_forest_id = forest.filter(|m| m.raster_id == raster.id).map_or(0, |m| m.id);
        let needs_create = !self.tiles.contains_key(&raster.id);
        let needs_forest_update = self
            .tiles
            .get(&raster.id)
            .is_some_and(|t| t.forest_id != want_forest_id);

        if needs_create || needs_forest_update {
            if let Some(old) = self.tiles.remove(&raster.id) {
                self.total_bytes = self.total_bytes.saturating_sub(old.bytes);
            }
            let w = raster.size[0] as u32;
            let h = raster.size[1] as u32;
            let usage = wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING;
            let height_tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("unified_terrain_height"),
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R32Float,
                usage,
                view_formats: &[],
            });
            let color_tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("unified_terrain_color"),
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage,
                view_formats: &[],
            });
            queue.write_texture(
                height_tex.as_image_copy(),
                bytemuck::cast_slice(&raster.heights),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(w * 4),
                    rows_per_image: Some(h),
                },
                wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
            );
            queue.write_texture(
                color_tex.as_image_copy(),
                &raster.colors,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(w * 4),
                    rows_per_image: Some(h),
                },
                wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
            );

            let (fw, fh, forest_pixels) = match forest.filter(|m| m.raster_id == raster.id) {
                Some(mask) => (mask.size[0] as u32, mask.size[1] as u32, mask.pixels.as_slice()),
                None => (1, 1, &[0u8][..]),
            };
            let forest_tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("unified_terrain_forest"),
                size: wgpu::Extent3d {
                    width: fw,
                    height: fh,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R8Unorm,
                usage,
                view_formats: &[],
            });
            queue.write_texture(
                forest_tex.as_image_copy(),
                forest_pixels,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(fw),
                    rows_per_image: Some(fh),
                },
                wgpu::Extent3d {
                    width: fw,
                    height: fh,
                    depth_or_array_layers: 1,
                },
            );

            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("unified_terrain_source_bg"),
                layout: &self.source_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(
                            &height_tex.create_view(&Default::default()),
                        ),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(
                            &color_tex.create_view(&Default::default()),
                        ),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(
                            &forest_tex.create_view(&Default::default()),
                        ),
                    },
                ],
            });
            let bytes = (w as usize * h as usize * 8) + (fw as usize * fh as usize);
            self.total_bytes += bytes;
            self.tiles.insert(
                raster.id,
                GpuTile {
                    _height: height_tex,
                    _color: color_tex,
                    _forest: forest_tex,
                    forest_id: want_forest_id,
                    bind_group,
                    last_used: self.tick,
                    bytes,
                },
            );
            // LRU evict oldest tiles if over GPU budget.
            while self.total_bytes > self.budget_bytes && self.tiles.len() > 2 {
                let oldest = self
                    .tiles
                    .iter()
                    .min_by_key(|(_, t)| t.last_used)
                    .map(|(&id, _)| id);
                if let Some(id) = oldest
                    && let Some(removed) = self.tiles.remove(&id)
                {
                    self.total_bytes = self.total_bytes.saturating_sub(removed.bytes);
                } else {
                    break;
                }
            }
        } else if let Some(tile) = self.tiles.get_mut(&raster.id) {
            tile.last_used = self.tick;
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
        rasters: &[Arc<Raster>],
        forest: Option<&Arc<atlas_forest::Mask>>,
        style: crate::atlas_style::AtlasStyle,
        light_jd: f64,
        contours: bool,
        bathymetry: bool,
        coastlines: bool,
        contour_width_px: f32,
        contour_interval_m: f32,
    ) {
        if rasters.is_empty() {
            return;
        }
        let count = rasters.len().min(MAX_TILES_PER_FRAME);

        // Build 72×36 5°-tile coverage mask so the global overview discards
        // cells already covered by higher-res 5° tiles.
        let mut coverage = [0u8; 72 * 36];
        let mut has_5deg_tiles = false;
        let mut finest_detail_bounds: Option<[f32; 4]> = None;

        for r in rasters.iter().take(count) {
            if let Some(core) = r.core {
                let span_lon = core[2] - core[0];
                let span_lat = core[3] - core[1];
                if (span_lon - 5.0).abs() < 0.05 && (span_lat - 5.0).abs() < 0.05 {
                    let cx = (((core[0] + 2.5 + 540.0) % 360.0) / 5.0).floor() as usize;
                    let cy = (((core[1] + 2.5 + 90.0) / 5.0).floor() as usize).min(35);
                    if cx < 72 && cy < 36 {
                        coverage[cy * 72 + cx] = 1;
                        has_5deg_tiles = true;
                    }
                }
            } else if (r.bounds[2] - r.bounds[0]) < 180.0 {
                finest_detail_bounds = Some(r.bounds);
            }
        }
        queue.write_texture(
            self.coverage_tex.as_image_copy(),
            &coverage,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(72),
                rows_per_image: Some(36),
            },
            wgpu::Extent3d {
                width: 72,
                height: 36,
                depth_or_array_layers: 1,
            },
        );

        let style = style.normalized();
        let light = style.light(light_jd);
        let flags = u32::from(crate::theme::tanaka_contours())
            | ((crate::theme::terrain_analysis_mode_u8() as u32 & 3) << 1);
        let major_col = super::gpu_color::rgba(crate::theme::hot_color());
        let minor_col = super::gpu_color::rgba(crate::theme::contour_color());

        let mut draw_specs: Vec<(u64, u32, u32, u32)> = Vec::with_capacity(count);
        for (slot, r) in rasters.iter().take(count).enumerate() {
            self.ensure_tile(device, queue, r, forest);
            let draw_bounds = r.core.unwrap_or(r.bounds);
            let center_lon = ((draw_bounds[0] + draw_bounds[2]) * 0.5) as f64;
            let center_lat = ((draw_bounds[1] + draw_bounds[3]) * 0.5) as f64;
            let tile_origin = geo_to_ecef(center_lat, center_lon, 0.0, proj.radius_m);
            let (east, north, up) = local_enu_basis(center_lat, center_lon);
            let (s0, c0) = center_lat.to_radians().sin_cos();

            // Adaptive mesh density: enough triangles to follow both curvature and height detail.
            let is_global = (draw_bounds[2] - draw_bounds[0]) > 180.0;
            let limit = if is_global { 512usize } else { 1024usize };
            let px_per_deg =
                (111_320.0 / proj.meters_per_pixel().max(0.01)).clamp(1.0, 200_000.0) as f32;
            let span_lon = (draw_bounds[2] - draw_bounds[0]).abs();
            let span_lat = (draw_bounds[3] - draw_bounds[1]).abs();
            let cols = (r.size[0] - 1)
                .min(limit)
                .min(((span_lon * px_per_deg).ceil() as usize).max(if is_global { 128 } else { 24 }))
                .max(4) as u32;
            let rows = (r.size[1] - 1)
                .min(limit)
                .min(((span_lat * px_per_deg).ceil() as usize).max(if is_global { 64 } else { 24 }))
                .max(4) as u32;

            let (exclude_mode, exclude_bounds) = if is_global {
                if let Some(detail) = finest_detail_bounds {
                    (1.0_f32, detail)
                } else if has_5deg_tiles {
                    (2.0_f32, [0.0; 4])
                } else {
                    (0.0_f32, [0.0; 4])
                }
            } else if r.core.is_some()
                && let Some(detail) = finest_detail_bounds
            {
                (1.0_f32, detail)
            } else {
                (0.0_f32, [0.0; 4])
            };

            let forest_on = forest.is_some_and(|m| m.raster_id == r.id);
            let u = UnifiedTerrainUniforms {
                cam: proj.rtc_uniforms(tile_origin),
                region: r.bounds,
                core: draw_bounds,
                tile_east: [
                    east.x as f32,
                    east.y as f32,
                    east.z as f32,
                    center_lon.to_radians() as f32,
                ],
                tile_north: [
                    north.x as f32,
                    north.y as f32,
                    north.z as f32,
                    center_lat.to_radians() as f32,
                ],
                tile_up: [up.x as f32, up.y as f32, up.z as f32, c0 as f32],
                grid: [
                    s0 as f32,
                    cols as f32,
                    rows as f32,
                    u32::from(coastlines) as f32,
                ],
                style: [
                    contour_interval_m.max(1.0),
                    contour_width_px,
                    u32::from(contours) as f32,
                    u32::from(bathymetry) as f32,
                ],
                appearance: [
                    style.color_richness,
                    style.relief_strength,
                    u32::from(forest_on) as f32,
                    flags as f32,
                ],
                light,
                major: major_col,
                minor: minor_col,
                exclude: exclude_bounds,
                extra: [
                    exclude_mode,
                    u32::from(r.core.is_some()) as f32,
                    1.0,
                    0.0,
                ],
            };
            queue.write_buffer(
                &self.uniform_buf,
                slot as u64 * UNIFORM_STRIDE,
                bytemuck::bytes_of(&u),
            );
            draw_specs.push((r.id, slot as u32, cols, rows));
        }

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("unified_terrain_pass"),
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
        for (raster_id, slot, cols, rows) in draw_specs {
            let Some(tile) = self.tiles.get(&raster_id) else {
                continue;
            };
            pass.set_bind_group(0, &self.uniform_bind, &[slot * (UNIFORM_STRIDE as u32)]);
            pass.set_bind_group(1, &tile.bind_group, &[]);
            pass.draw(0..(cols + 1) * 2, 0..rows);
        }
    }
}

pub struct UnifiedTerrainCallback {
    pub proj: MapProjection,
    pub rasters: Vec<Arc<Raster>>,
    pub forest: Option<Arc<atlas_forest::Mask>>,
    pub style: crate::atlas_style::AtlasStyle,
    pub light_jd: f64,
    pub contours: bool,
    pub bathymetry: bool,
    pub coastlines: bool,
    pub contour_width_px: f32,
    pub contour_interval_m: f32,
}

impl UnifiedTerrainCallback {
    pub fn into_paint_callback(self) -> egui::PaintCallback {
        let rect = self.proj.viewport;
        egui_wgpu::Callback::new_paint_callback(rect, self)
    }
}

impl egui_wgpu::CallbackTrait for UnifiedTerrainCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if self.rasters.is_empty() {
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
        if let Some(terrain) = resources.get_mut::<UnifiedTerrainResources>() {
            terrain.encode_into_scene(
                device,
                queue,
                encoder,
                &color_view,
                &depth_view,
                &self.proj,
                &self.rasters,
                self.forest.as_ref(),
                self.style,
                self.light_jd,
                self.contours,
                self.bathymetry,
                self.coastlines,
                self.contour_width_px,
                self.contour_interval_m,
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

const UNIFIED_TERRAIN_WGSL: &str = concat!(
    include_str!("unified_camera.wgsl"),
    "\n",
    include_str!("unified_terrain.wgsl"),
    "\n",
    include_str!("atlas_lighting.wgsl")
);
