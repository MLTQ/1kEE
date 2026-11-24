use std::collections::HashMap;

use eframe::{egui_wgpu, wgpu};

use crate::model::{GeoPoint, GlobeViewState};
use crate::theme;

use super::super::{gpu_color, srtm_focus_cache};
use super::projection::project_local;
use super::{LocalLayout, visual_half_extent_for_zoom};

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct DissolveUniforms {
    viewport: [f32; 4],
    contour_color: [f32; 4],
    hot_color: [f32; 4],
    params: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
struct DissolveTileInstance {
    nw: [f32; 2],
    ne: [f32; 2],
    se: [f32; 2],
    sw: [f32; 2],
    removed: u32,
    rank_offset: u32,
}

struct DissolveGpuResources {
    pipelines: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
    pipeline_layout: wgpu::PipelineLayout,
    shader: wgpu::ShaderModule,
    uniform_buf: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    _ranks_tex: wgpu::Texture,
    instance_buf: Option<wgpu::Buffer>,
    instance_cap: usize,
    instance_count: u32,
}

impl DissolveGpuResources {
    fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("local_dissolve_shader"),
            source: gpu_color::shader(include_str!("dissolve.wgsl")),
        });
        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("local_dissolve_uniforms"),
            size: std::mem::size_of::<DissolveUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let ranks_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("local_dissolve_ranks"),
            size: wgpu::Extent3d {
                width: GRID as u32,
                height: GRID as u32,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R16Uint,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let ranks_u16: Vec<u16> = base_ranks().iter().map(|&r| r as u16).collect();
        queue.write_texture(
            ranks_tex.as_image_copy(),
            bytemuck::cast_slice(&ranks_u16),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some((GRID * std::mem::size_of::<u16>()) as u32),
                rows_per_image: Some(GRID as u32),
            },
            wgpu::Extent3d {
                width: GRID as u32,
                height: GRID as u32,
                depth_or_array_layers: 1,
            },
        );
        let ranks_view = ranks_tex.create_view(&Default::default());
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("local_dissolve_bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(
                            std::mem::size_of::<DissolveUniforms>() as u64,
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
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("local_dissolve_bg"),
            layout: &bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&ranks_view),
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("local_dissolve_pipeline_layout"),
            bind_group_layouts: &[&bgl],
            push_constant_ranges: &[],
        });
        Self {
            pipelines: HashMap::new(),
            pipeline_layout,
            shader,
            uniform_buf,
            bind_group,
            _ranks_tex: ranks_tex,
            instance_buf: None,
            instance_cap: 0,
            instance_count: 0,
        }
    }

    fn ensure_pipeline(&mut self, device: &wgpu::Device, format: wgpu::TextureFormat) {
        if self.pipelines.contains_key(&format) {
            return;
        }
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("local_dissolve_pipeline"),
            layout: Some(&self.pipeline_layout),
            vertex: wgpu::VertexState {
                module: &self.shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<DissolveTileInstance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x2,
                        1 => Float32x2,
                        2 => Float32x2,
                        3 => Float32x2,
                        4 => Uint32,
                        5 => Uint32,
                    ],
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &self.shader,
                entry_point: Some("fs_main"),
                compilation_options: gpu_color::fragment_options(format),
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
        self.pipelines.insert(format, pipeline);
    }
}

struct DissolveCallback {
    uniforms: DissolveUniforms,
    tiles: Vec<DissolveTileInstance>,
}

impl egui_wgpu::CallbackTrait for DissolveCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let format = resources
            .get::<super::super::contour_pass::ContourPassResources>()
            .map_or(wgpu::TextureFormat::Rgba8Unorm, |r| r.format);
        if resources.get::<DissolveGpuResources>().is_none() {
            resources.insert(DissolveGpuResources::new(device, queue));
        }
        let res = resources.get_mut::<DissolveGpuResources>().unwrap();
        res.ensure_pipeline(device, format);
        queue.write_buffer(&res.uniform_buf, 0, bytemuck::bytes_of(&self.uniforms));
        res.instance_count = self.tiles.len() as u32;
        if !self.tiles.is_empty() {
            if self.tiles.len() > res.instance_cap {
                let cap = self.tiles.len().next_power_of_two().max(64);
                res.instance_buf = Some(device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("local_dissolve_instances"),
                    size: (cap * std::mem::size_of::<DissolveTileInstance>()) as u64,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }));
                res.instance_cap = cap;
            }
            if let Some(buf) = &res.instance_buf {
                queue.write_buffer(buf, 0, bytemuck::cast_slice(&self.tiles));
            }
        }
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        resources: &egui_wgpu::CallbackResources,
    ) {
        let format = resources
            .get::<super::super::contour_pass::ContourPassResources>()
            .map_or(wgpu::TextureFormat::Rgba8Unorm, |r| r.format);
        let Some(res) = resources.get::<DissolveGpuResources>() else {
            return;
        };
        if res.instance_count == 0 {
            return;
        }
        let (Some(pipeline), Some(buf)) = (res.pipelines.get(&format), &res.instance_buf) else {
            return;
        };
        render_pass.set_pipeline(pipeline);
        render_pass.set_bind_group(0, &res.bind_group, &[]);
        render_pass.set_vertex_buffer(0, buf.slice(..));
        render_pass.draw(0..4, 0..res.instance_count);
    }
}

fn collect_pending_tiles(
    clip_rect: egui::Rect,
    ppp: f32,
    layout: &LocalLayout,
    view: &GlobeViewState,
    viewport_center: GeoPoint,
    render_zoom: f32,
    radius: i32,
    ready_buckets: &std::collections::HashSet<(i32, i32)>,
    progress: &std::collections::HashMap<(i32, i32), f32>,
    half_extent_override: Option<f32>,
) -> Vec<DissolveTileInstance> {
    let half_extent =
        half_extent_override.unwrap_or_else(|| srtm_focus_cache::half_extent_for_zoom(render_zoom));
    let bucket_step = half_extent * 0.45;
    let visual_half = visual_half_extent_for_zoom(view.local_zoom);
    let km_per_deg_lat = 111.32f32;
    let km_per_deg_lon = km_per_deg_lat * viewport_center.lat.to_radians().cos().abs().max(0.2);
    let extent_x_km = (visual_half * km_per_deg_lon).max(1.0);
    let extent_y_km = (visual_half * km_per_deg_lat).max(1.0);

    let center_lat_b = (viewport_center.lat / bucket_step).round() as i32;
    let center_lon_b = (viewport_center.lon / bucket_step).round() as i32;
    let half = if half_extent_override.is_some() {
        half_extent
    } else {
        bucket_step * 0.5
    };

    let mut tiles = Vec::new();
    for dlat in -radius..=radius {
        for dlon in -radius..=radius {
            let lat_b = center_lat_b + dlat;
            let lon_b = center_lon_b + dlon;
            if ready_buckets.contains(&(lat_b, lon_b)) {
                continue;
            }
            let removed = completed_cells(progress.get(&(lat_b, lon_b)).copied().unwrap_or(0.0));
            if removed == GRID * GRID {
                continue;
            }

            let tile_lat = (lat_b as f32 * bucket_step).clamp(-89.9, 89.9);
            let tile_lon = lon_b as f32 * bucket_step;

            // Project the 4 geo corners -> screen space (NW, NE, SE, SW).
            let geo_corners = [
                GeoPoint {
                    lat: tile_lat + half,
                    lon: tile_lon - half,
                },
                GeoPoint {
                    lat: tile_lat + half,
                    lon: tile_lon + half,
                },
                GeoPoint {
                    lat: tile_lat - half,
                    lon: tile_lon + half,
                },
                GeoPoint {
                    lat: tile_lat - half,
                    lon: tile_lon - half,
                },
            ];
            let mut sc = [egui::Pos2::ZERO; 4];
            let mut ok = true;
            for (k, &c) in geo_corners.iter().enumerate() {
                if let Some(p) = project_local(
                    layout,
                    view,
                    viewport_center,
                    c,
                    0.0,
                    extent_x_km,
                    extent_y_km,
                ) {
                    sc[k] = p.pos;
                } else {
                    ok = false;
                    break;
                }
            }
            if !ok || !clip_rect.intersects(egui::Rect::from_points(&sc)) {
                continue;
            }
            let rank_offset = (tile_hash(lat_b, lon_b) % (GRID * GRID) as u64) as u32;
            tiles.push(DissolveTileInstance {
                nw: [sc[0].x * ppp, sc[0].y * ppp],
                ne: [sc[1].x * ppp, sc[1].y * ppp],
                se: [sc[2].x * ppp, sc[2].y * ppp],
                sw: [sc[3].x * ppp, sc[3].y * ppp],
                removed: removed as u32,
                rank_offset,
            });
        }
    }
    tiles
}

/// Each tile has 10,000 deterministically shuffled cells: one per 0.01% of
/// measured workflow completion. Only real loading progress removes cells.
/// Time affects brightness, never the completed-cell count or ordering.
pub(super) fn draw_tile_pulse_grid(
    painter: &egui::Painter,
    layout: &LocalLayout,
    view: &GlobeViewState,
    viewport_center: GeoPoint,
    render_zoom: f32,
    radius: i32,
    time: f64,
    ready_buckets: &std::collections::HashSet<(i32, i32)>,
    progress: &std::collections::HashMap<(i32, i32), f32>,
    half_extent_override: Option<f32>,
) {
    let ppp = painter.ctx().pixels_per_point();
    let tiles = collect_pending_tiles(
        painter.clip_rect(),
        ppp,
        layout,
        view,
        viewport_center,
        render_zoom,
        radius,
        ready_buckets,
        progress,
        half_extent_override,
    );
    if tiles.is_empty() {
        return;
    }

    // Brightness indicates activity without advancing completion.
    let breath = ((time as f32 * std::f32::consts::TAU / 4.5).sin() * 0.5 + 0.5) * 0.35 + 0.65;
    let rect = painter.clip_rect();
    let uniforms = DissolveUniforms {
        viewport: [
            rect.min.x * ppp,
            rect.min.y * ppp,
            (rect.width() * ppp).max(1.0),
            (rect.height() * ppp).max(1.0),
        ],
        contour_color: gpu_color::rgba(theme::contour_color()),
        hot_color: gpu_color::rgba(theme::hot_color()),
        params: [breath, ppp, 0.0, 0.0],
    };
    painter.add(egui_wgpu::Callback::new_paint_callback(
        rect,
        DissolveCallback { uniforms, tiles },
    ));
}

pub(super) fn draw_frame(painter: &egui::Painter, rect: egui::Rect) {
    painter.rect_stroke(
        rect.shrink(6.0),
        12.0,
        egui::Stroke::new(0.7, theme::topo_color().gamma_multiply(0.45)),
        egui::StrokeKind::Outside,
    );

    for &(x, y, x_dir, y_dir) in &[
        (rect.left() + 18.0, rect.top() + 18.0, 28.0, 16.0),
        (rect.right() - 18.0, rect.top() + 18.0, -28.0, 16.0),
        (rect.left() + 18.0, rect.bottom() - 18.0, 28.0, -16.0),
        (rect.right() - 18.0, rect.bottom() - 18.0, -28.0, -16.0),
    ] {
        painter.line_segment(
            [egui::pos2(x, y), egui::pos2(x + x_dir, y)],
            egui::Stroke::new(1.0, theme::topo_color()),
        );
        painter.line_segment(
            [egui::pos2(x, y), egui::pos2(x, y + y_dir)],
            egui::Stroke::new(1.0, theme::topo_color()),
        );
    }
}

// ── tile-dissolve helpers ──────────────────────────────────────────────────

/// Stable per-tile seed from bucket coordinates.
#[inline]
fn tile_hash(lat_b: i32, lon_b: i32) -> u64 {
    let a = (lat_b as u64) & 0xFFFF_FFFF;
    let b = (lon_b as u64) & 0xFFFF_FFFF;
    a.wrapping_mul(2_654_435_761)
        .wrapping_add(b.wrapping_mul(2_246_822_519))
        .wrapping_mul(6_364_136_223_846_793_005)
}

const GRID: usize = 100;

fn completed_cells(progress: f32) -> usize {
    if !progress.is_finite() {
        return 0;
    }
    (progress.clamp(0.0, 1.0) * (GRID * GRID) as f32).floor() as usize
}

fn base_ranks() -> &'static [usize] {
    // Sort only once, rather than sorting 10,000 cells for every tile/frame.
    // Rotating the ranks keeps each tile's pattern stable and the exact count
    // of removed cells intact without allocating another per-tile array.
    static RANKS: std::sync::OnceLock<Vec<usize>> = std::sync::OnceLock::new();
    RANKS.get_or_init(|| {
        let mut order: Vec<usize> = (0..GRID * GRID).collect();
        order.sort_unstable_by_key(|&i| {
            let mut x = (i as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
            x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            x ^ (x >> 31)
        });
        let mut ranks = vec![0; GRID * GRID];
        for (rank, index) in order.into_iter().enumerate() {
            ranks[index] = rank;
        }
        ranks
    })
}

/// A permutation gives exactly N dissolved cells, unlike random float
/// thresholds which only approximate a percentage and can finish early.
#[cfg(test)]
fn cell_ranks(seed: u64) -> impl Iterator<Item = usize> {
    let offset = (seed % (GRID * GRID) as u64) as usize;
    base_ranks()
        .iter()
        .map(move |rank| (rank + offset) % (GRID * GRID))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn painted_cells_hold_across_the_old_timer_cycle_and_clear_on_readiness() {
        let collect = |fraction, ready| {
            let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
            let focus = GeoPoint { lat: 0.0, lon: 0.0 };
            let view = GlobeViewState::from_focus(focus);
            let ready = if ready {
                std::collections::HashSet::from([(0, 0)])
            } else {
                std::collections::HashSet::new()
            };
            collect_pending_tiles(
                rect,
                1.0,
                &super::super::layout(rect),
                &view,
                focus,
                25.0,
                0,
                &ready,
                &std::collections::HashMap::from([((0, 0), fraction)]),
                None,
            )
        };
        let paint_callbacks = |time, fraction, ready| {
            let ctx = egui::Context::default();
            let output = ctx.run(egui::RawInput::default(), |ctx| {
                let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
                let painter = ctx.layer_painter(egui::LayerId::background());
                let focus = GeoPoint { lat: 0.0, lon: 0.0 };
                let view = GlobeViewState::from_focus(focus);
                let ready = if ready {
                    std::collections::HashSet::from([(0, 0)])
                } else {
                    std::collections::HashSet::new()
                };
                draw_tile_pulse_grid(
                    &painter,
                    &super::super::layout(rect),
                    &view,
                    focus,
                    25.0,
                    0,
                    time,
                    &ready,
                    &std::collections::HashMap::from([((0, 0), fraction)]),
                    None,
                );
            });
            output
                .shapes
                .into_iter()
                .filter(|shape| matches!(shape.shape, egui::Shape::Callback(_)))
                .count()
        };
        let stalled = collect(0.5, false);
        assert_eq!(stalled.len(), 1);
        assert_eq!(GRID * GRID - stalled[0].removed as usize, 5_000);
        assert_eq!(paint_callbacks(0.0, 0.5, false), 1);
        assert_eq!(paint_callbacks(6.9, 0.5, false), 1);
        assert_eq!(paint_callbacks(7.1, 0.5, false), 1);
        assert_eq!(paint_callbacks(100.0, 0.5, false), 1);
        assert_eq!(GRID * GRID - collect(0.75, false)[0].removed as usize, 2_500);
        assert_eq!(GRID * GRID - collect(0.99, false)[0].removed as usize, 100);
        assert!(collect(1.0, false).is_empty());
        assert_eq!(paint_callbacks(100.0, 1.0, false), 0);
        assert!(collect(0.0, true).is_empty());
        assert_eq!(paint_callbacks(100.0, 0.0, true), 0);
    }

    #[test]
    fn dissolve_tracks_completed_work_exactly_without_reappearing() {
        let ranks: Vec<_> = cell_ranks(tile_hash(5467, -16911)).collect();
        let mut previous = std::collections::HashSet::new();
        for progress in [0.0, 0.25, 0.50, 0.75, 0.99, 1.0] {
            let gone: std::collections::HashSet<_> = ranks
                .iter()
                .enumerate()
                .filter_map(|(cell, &rank)| (rank < completed_cells(progress)).then_some(cell))
                .collect();
            assert_eq!(gone.len(), completed_cells(progress));
            assert!(previous.is_subset(&gone));
            previous = gone;
        }
        assert_eq!(previous.len(), 10_000);
        assert_eq!(
            cell_ranks(tile_hash(5467, -16911)).collect::<Vec<_>>(),
            ranks
        );
        assert_ne!(
            cell_ranks(tile_hash(5467, -16910)).collect::<Vec<_>>(),
            ranks
        );
    }

    #[test]
    fn queued_unknown_and_stalled_work_do_not_claim_completion() {
        assert_eq!(completed_cells(0.0), 0);
        assert_eq!(completed_cells(f32::NAN), 0);
        assert_eq!(completed_cells(f32::INFINITY), 0);
        assert_eq!(completed_cells(-1.0), 0);
        assert_eq!(completed_cells(0.99), 9_900);
        assert_eq!(completed_cells(5.0), 10_000);
    }
}
