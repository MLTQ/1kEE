//! GPU-instanced contour renderer for the local terrain scene.
//!
//! The globe has had one of these since `contour_pass.rs`; this is its
//! tangent-plane sibling. It exists because 3DEP put two orders of magnitude
//! more geometry in front of the local scene than SRTM ever did — a single
//! bucket-10 tile is ~4.3 M points — and the CPU path could not keep up.
//!
//! # Why per-tile buffers
//!
//! `contour_pass` keeps one buffer per layer and rebuilds it whenever the
//! merged contour `Arc` changes. That is fine on the globe, where tile sets
//! change rarely. Locally, a tile lands every few seconds while navigating, and
//! rebuilding tens of millions of instances each time would stall harder than
//! the CPU renderer it replaces.
//!
//! So geometry is keyed and uploaded **per source tile**. Each tile's
//! `Arc<Vec<ContourPath>>` is stable once loaded, so its instances are built
//! once, uploaded once, and left alone. A new tile allocates its own buffer and
//! disturbs nothing. Per frame the cost is one uniform write plus one instanced
//! draw per resident tile.
//!
//! # Why there is no point cap
//!
//! There is a *memory* budget instead. The CPU path had to cap points because
//! every one of them cost projection and tessellation time each frame; here
//! they cost only VRAM, and drawing is flat in the number of points. The budget
//! evicts least-recently-used tiles, so the ceiling is "how much geometry do
//! you want resident", not "how much can be redrawn in 16 ms".

use eframe::egui_wgpu;
use eframe::wgpu;
use eframe::wgpu::util::DeviceExt;
use rayon::prelude::*;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use super::contour_asset::ContourPath;
use super::globe_pass::color_to_linear;
use super::local_terrain_scene::projection::LocalProjectionParams;
use crate::settings_store;

const LOCAL_CONTOUR_WGSL: &str = include_str!("local_contour_lines.wgsl");

/// wgpu requires dynamic uniform offsets to be 256-aligned on most hardware.
const UNIFORM_STRIDE: u64 = 256;

/// The local scene draws contours twice: once beneath the opaque elevation fill
/// and once above it. Each pass gets its own uniform slot because they differ in
/// fade and stroke width.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum LocalContourPass {
    /// Drawn before the elevation fill, which then occludes it.
    Background,
    /// Drawn after the fill so lines stay visible on top.
    Surface,
}

const PASS_COUNT: usize = 2;

impl LocalContourPass {
    fn slot(self) -> u32 {
        match self {
            LocalContourPass::Background => 0,
            LocalContourPass::Surface => 1,
        }
    }
}

/// Identity of one source contour tile.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct LocalTileId {
    pub zoom_bucket: i32,
    pub lat_bucket: i32,
    pub lon_bucket: i32,
}

/// One polyline segment. Endpoints are raw geography — the shader applies the
/// whole local transform — so a tile's instances stay valid under any camera
/// motion and are never rebuilt for a pan, rotate or zoom.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct LocalSegmentInstance {
    /// (lon, lat, elevation_m)
    a: [f32; 3],
    b: [f32; 3],
    color: [u8; 4],
    /// 1 for major contours, 0 for minor. The two carry different stroke
    /// widths, and the CPU width formula ends in a `max()` floor, so the widths
    /// are computed host-side and selected per instance rather than scaled from
    /// one width by a multiplier.
    major: u32,
}

const _: () = assert!(std::mem::size_of::<LocalSegmentInstance>() == 32);

/// Must stay byte-for-byte in sync with `Uniforms` in local_contour_lines.wgsl.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct LocalContourUniforms {
    focus_lon: f32,
    focus_lat: f32,
    x_factor: f32,
    y_factor: f32,
    z_factor: f32,
    yaw_cos: f32,
    yaw_sin: f32,
    pitch_cos: f32,
    pitch_sin: f32,
    focus_center_x: f32,
    focus_center_y: f32,
    horizontal_scale: f32,
    ground_pitch_scale: f32,
    ground_depth_scale: f32,
    elevation_pitch_scale: f32,
    elevation_depth_scale: f32,
    viewport_min_x: f32,
    viewport_min_y: f32,
    viewport_size_x: f32,
    viewport_size_y: f32,
    stroke_half_px_minor: f32,
    stroke_half_px_major: f32,
    feather_px_minor: f32,
    feather_px_major: f32,
    pixels_per_point: f32,
    alpha: f32,
    _pad0: f32,
    _pad1: f32,
}

const _: () = assert!(std::mem::size_of::<LocalContourUniforms>() == 112);

#[inline]
fn linear_u8(c: egui::Color32) -> [u8; 4] {
    let mut l = color_to_linear(c);
    l[3] = (c.a() as f32 / 255.0).powf(2.2);
    l.map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8)
}

// ── Instance building ────────────────────────────────────────────────────────

/// A tile's built instances, plus the version that produced them.
#[derive(Clone)]
pub struct LocalTileBatch {
    pub id: LocalTileId,
    pub version: u64,
    pub instances: Arc<Vec<LocalSegmentInstance>>,
}

#[derive(Default)]
struct TileInstances {
    current: Option<(u64, Arc<Vec<LocalSegmentInstance>>)>,
    building: Option<u64>,
}

fn instance_cache() -> &'static Mutex<HashMap<LocalTileId, TileInstances>> {
    static CACHE: OnceLock<Mutex<HashMap<LocalTileId, TileInstances>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Version of a tile's geometry: its `Arc` identity folded with the palette.
/// Contours are immutable once loaded, so pointer identity is a sound key.
fn version_key(contours: &Arc<Vec<ContourPath>>, palette: u64) -> u64 {
    let ptr = Arc::as_ptr(contours) as *const () as u64;
    ptr ^ palette.rotate_left(17) ^ (contours.len() as u64).rotate_left(33)
}

/// Instances for one tile, building them in the background on first request.
///
/// Returns `None` until the build finishes; the scene simply draws the tiles
/// that are ready, which is the same progressive fill the cache already shows.
pub fn instances_for_tile(
    id: LocalTileId,
    contours: &Arc<Vec<ContourPath>>,
    palette: u64,
    ctx: &egui::Context,
    style_fn: impl Fn(&ContourPath) -> (egui::Color32, bool) + Send + Sync + 'static,
) -> Option<LocalTileBatch> {
    let version = version_key(contours, palette);

    {
        let mut guard = instance_cache().lock().ok()?;
        let entry = guard.entry(id).or_default();
        if let Some((v, instances)) = &entry.current
            && *v == version
        {
            return Some(LocalTileBatch {
                id,
                version,
                instances: instances.clone(),
            });
        }
        if entry.building == Some(version) {
            return None;
        }
        entry.building = Some(version);
    }

    let contours = contours.clone();
    let ctx = ctx.clone();
    if std::thread::Builder::new()
        .name("local-contour-instance-build".into())
        .spawn(move || {
            let built = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                puffin::profile_scope!("local_contour_instances_build");
                contours
                    .par_iter()
                    .flat_map_iter(|contour| {
                        let (color, is_major) = style_fn(contour);
                        let color = linear_u8(color);
                        let major = u32::from(is_major);
                        contour.points.windows(2).map(move |pair| LocalSegmentInstance {
                            a: [pair[0].lon, pair[0].lat, contour.elevation_m],
                            b: [pair[1].lon, pair[1].lat, contour.elevation_m],
                            color,
                            major,
                        })
                    })
                    .collect::<Vec<_>>()
            }));

            if let Ok(mut guard) = instance_cache().lock() {
                let entry = guard.entry(id).or_default();
                entry.building = None;
                // A panicked build releases the gate and leaves any previous
                // instances displayable rather than wedging the tile.
                if let Ok(instances) = built {
                    entry.current = Some((version, Arc::new(instances)));
                }
            }
            ctx.request_repaint();
        })
        .is_err()
        && let Ok(mut guard) = instance_cache().lock()
    {
        guard.entry(id).or_default().building = None;
    }
    None
}

/// Drop every cached instance set. Wired to the same cache reset as the other
/// terrain layers.
pub fn clear_instances() {
    if let Ok(mut guard) = instance_cache().lock() {
        guard.clear();
    }
}

// ── GPU resources ────────────────────────────────────────────────────────────

struct TileGpu {
    version: u64,
    instances: wgpu::Buffer,
    count: u32,
    bytes: u64,
    last_used: u64,
}

pub struct LocalContourPassResources {
    pipeline: wgpu::RenderPipeline,
    uniform_buf: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    tiles: HashMap<LocalTileId, TileGpu>,
    resident_bytes: u64,
    frame: u64,
}

/// Uploading a whole envelope in one frame would stall visibly, so a cold start
/// spreads it over consecutive frames. Tiles arrive gradually in normal use, so
/// this only bites on a jump to a fully-cached area.
const MAX_TILE_UPLOADS_PER_FRAME: usize = 3;

/// Mirror of the resident byte count, so the settings panel can report GPU use
/// without reaching into `CallbackResources` from outside a paint callback.
static RESIDENT_BYTES: AtomicU64 = AtomicU64::new(0);

/// Bytes of local contour geometry currently resident on the GPU.
pub fn resident_bytes() -> u64 {
    RESIDENT_BYTES.load(Ordering::Relaxed)
}

impl LocalContourPassResources {
    /// Create the pipeline and slotted uniform buffer. Call once from
    /// `DashboardApp::new`, next to `ContourPassResources::new`.
    pub fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("local_contour_pass_shader"),
            source: wgpu::ShaderSource::Wgsl(LOCAL_CONTOUR_WGSL.into()),
        });

        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("local_contour_uniforms"),
            size: UNIFORM_STRIDE * PASS_COUNT as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("local_contour_bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(
                        std::mem::size_of::<LocalContourUniforms>() as u64,
                    ),
                },
                count: None,
            }],
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("local_contour_bg"),
            layout: &bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &uniform_buf,
                    offset: 0,
                    size: wgpu::BufferSize::new(
                        std::mem::size_of::<LocalContourUniforms>() as u64
                    ),
                }),
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("local_contour_pl"),
            bind_group_layouts: &[&bgl],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("local_contour_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<LocalSegmentInstance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &[
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x3,
                            offset: 0,
                            shader_location: 0,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x3,
                            offset: 12,
                            shader_location: 1,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Unorm8x4,
                            offset: 24,
                            shader_location: 2,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Uint32,
                            offset: 28,
                            shader_location: 3,
                        },
                    ],
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        Self {
            pipeline,
            uniform_buf,
            bind_group,
            tiles: HashMap::new(),
            resident_bytes: 0,
            frame: 0,
        }
    }

    /// Evict least-recently-drawn tiles until resident geometry fits the
    /// configured budget. Tiles drawn this frame are never evicted: dropping one
    /// would only force it to be re-uploaded on the next frame.
    fn enforce_budget(&mut self, budget_bytes: u64, frame: u64) {
        if self.resident_bytes <= budget_bytes {
            return;
        }
        let mut evictable: Vec<(LocalTileId, u64, u64)> = self
            .tiles
            .iter()
            .filter(|(_, gpu)| gpu.last_used != frame)
            .map(|(id, gpu)| (*id, gpu.last_used, gpu.bytes))
            .collect();
        evictable.sort_unstable_by_key(|(_, last_used, _)| *last_used);

        for (id, _, bytes) in evictable {
            if self.resident_bytes <= budget_bytes {
                break;
            }
            if self.tiles.remove(&id).is_some() {
                self.resident_bytes = self.resident_bytes.saturating_sub(bytes);
            }
        }
    }

    /// Bytes of contour geometry currently held on the GPU.
    fn publish_resident_bytes(&self) {
        RESIDENT_BYTES.store(self.resident_bytes, Ordering::Relaxed);
    }
}

// ── Per-frame callback ───────────────────────────────────────────────────────

pub struct LocalContourCallback {
    pass: LocalContourPass,
    batches: Vec<LocalTileBatch>,
    uniforms: LocalContourUniforms,
}

impl LocalContourCallback {
    /// Build a callback for one of the scene's two contour passes.
    ///
    /// `stroke_width_px` and `alpha` land only in uniforms, so changing either
    /// never rebuilds or re-uploads geometry.
    pub fn new(
        pass: LocalContourPass,
        batches: Vec<LocalTileBatch>,
        params: &LocalProjectionParams,
        alpha: f32,
        stroke_width_px_minor: f32,
        stroke_width_px_major: f32,
        pixels_per_point: f32,
    ) -> Self {
        Self {
            pass,
            batches,
            uniforms: LocalContourUniforms {
                focus_lon: params.focus_lon,
                focus_lat: params.focus_lat,
                x_factor: params.x_factor,
                y_factor: params.y_factor,
                z_factor: params.z_factor,
                yaw_cos: params.yaw_cos,
                yaw_sin: params.yaw_sin,
                pitch_cos: params.pitch_cos,
                pitch_sin: params.pitch_sin,
                focus_center_x: params.focus_center_x,
                focus_center_y: params.focus_center_y,
                horizontal_scale: params.horizontal_scale,
                ground_pitch_scale: params.ground_pitch_scale,
                ground_depth_scale: params.ground_depth_scale,
                elevation_pitch_scale: params.elevation_pitch_scale,
                elevation_depth_scale: params.elevation_depth_scale,
                // Filled in by `into_paint_callback` once the rect is known.
                viewport_min_x: 0.0,
                viewport_min_y: 0.0,
                viewport_size_x: 1.0,
                viewport_size_y: 1.0,
                stroke_half_px_minor: stroke_width_px_minor * 0.5,
                stroke_half_px_major: stroke_width_px_major * 0.5,
                feather_px_minor: feather_px(stroke_width_px_minor),
                feather_px_major: feather_px(stroke_width_px_major),
                pixels_per_point,
                // Same gamma-space→linear-space correction the CPU path applied
                // through `gamma_multiply`.
                alpha: alpha.powf(2.2),
                _pad0: 0.0,
                _pad1: 0.0,
            },
        }
    }

    /// egui-wgpu executes paint callbacks with the GPU viewport set to `rect`
    /// (in physical pixels), so the NDC transform must be relative to it.
    pub fn into_paint_callback(mut self, rect: egui::Rect) -> egui::PaintCallback {
        let ppp = self.uniforms.pixels_per_point;
        self.uniforms.viewport_min_x = rect.min.x * ppp;
        self.uniforms.viewport_min_y = rect.min.y * ppp;
        self.uniforms.viewport_size_x = (rect.width() * ppp).max(1.0);
        self.uniforms.viewport_size_y = (rect.height() * ppp).max(1.0);
        egui_wgpu::Callback::new_paint_callback(rect, self)
    }
}

/// Match `contour_pass`'s narrow anti-alias fringe so GPU-drawn local contours
/// keep the same weight as the globe's at equal widths.
#[inline]
fn feather_px(stroke_width_px: f32) -> f32 {
    (stroke_width_px * 0.25).clamp(0.5, 1.0)
}

impl egui_wgpu::CallbackTrait for LocalContourCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen_descriptor: &egui_wgpu::ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(res) = resources.get_mut::<LocalContourPassResources>() else {
            return Vec::new();
        };

        queue.write_buffer(
            &res.uniform_buf,
            self.pass.slot() as u64 * UNIFORM_STRIDE,
            bytemuck::bytes_of(&self.uniforms),
        );

        // Only the first pass of a frame advances the clock and uploads; the
        // second draws exactly the same geometry with different uniforms.
        if self.pass == LocalContourPass::Background {
            res.frame = res.frame.wrapping_add(1);
        }
        let frame = res.frame;

        let mut uploads = 0usize;
        for batch in &self.batches {
            if batch.instances.is_empty() {
                continue;
            }
            let stale = res
                .tiles
                .get(&batch.id)
                .map(|gpu| gpu.version != batch.version)
                .unwrap_or(true);

            if stale {
                if uploads >= MAX_TILE_UPLOADS_PER_FRAME {
                    continue;
                }
                uploads += 1;
                puffin::profile_scope!("local_contour_tile_upload");
                let contents: &[u8] = bytemuck::cast_slice(&batch.instances);
                let bytes = contents.len() as u64;
                let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("local_contour_tile_instances"),
                    contents,
                    usage: wgpu::BufferUsages::VERTEX,
                });
                if let Some(previous) = res.tiles.insert(
                    batch.id,
                    TileGpu {
                        version: batch.version,
                        instances: buffer,
                        count: batch.instances.len() as u32,
                        bytes,
                        last_used: frame,
                    },
                ) {
                    res.resident_bytes = res.resident_bytes.saturating_sub(previous.bytes);
                }
                res.resident_bytes += bytes;
            } else if let Some(gpu) = res.tiles.get_mut(&batch.id) {
                gpu.last_used = frame;
            }
        }

        let budget = settings_store::local_contour_vram_budget_bytes();
        res.enforce_budget(budget, frame);
        res.publish_resident_bytes();
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        resources: &egui_wgpu::CallbackResources,
    ) {
        let Some(res) = resources.get::<LocalContourPassResources>() else {
            return;
        };
        render_pass.set_pipeline(&res.pipeline);
        render_pass.set_bind_group(
            0,
            &res.bind_group,
            &[self.pass.slot() * UNIFORM_STRIDE as u32],
        );
        for batch in &self.batches {
            let Some(gpu) = res.tiles.get(&batch.id) else {
                continue;
            };
            if gpu.count == 0 {
                continue;
            }
            render_pass.set_vertex_buffer(0, gpu.instances.slice(..));
            render_pass.draw(0..6, 0..gpu.count);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_uniform_block_matches_the_shader_layout() {
        // 28 f32s, and a multiple of 16 so it satisfies uniform alignment.
        assert_eq!(std::mem::size_of::<LocalContourUniforms>(), 28 * 4);
        assert_eq!(std::mem::size_of::<LocalContourUniforms>() % 16, 0);
        assert!(std::mem::size_of::<LocalContourUniforms>() as u64 <= UNIFORM_STRIDE);
    }

    #[test]
    fn the_shader_declares_the_same_uniform_fields_in_the_same_order() {
        // A silent reorder here would misproject every contour, so the field
        // order is checked against the WGSL source rather than trusted.
        let block = LOCAL_CONTOUR_WGSL
            .split_once("struct Uniforms {")
            .expect("uniform struct")
            .1
            .split_once('}')
            .expect("uniform struct end")
            .0;
        let shader_fields: Vec<&str> = block
            .lines()
            .filter_map(|line| line.trim().split(':').next())
            .filter(|name| !name.is_empty() && !name.starts_with("//"))
            .collect();
        let rust_fields = [
            "focus_lon",
            "focus_lat",
            "x_factor",
            "y_factor",
            "z_factor",
            "yaw_cos",
            "yaw_sin",
            "pitch_cos",
            "pitch_sin",
            "focus_center_x",
            "focus_center_y",
            "horizontal_scale",
            "ground_pitch_scale",
            "ground_depth_scale",
            "elevation_pitch_scale",
            "elevation_depth_scale",
            "viewport_min_x",
            "viewport_min_y",
            "viewport_size_x",
            "viewport_size_y",
            "stroke_half_px_minor",
            "stroke_half_px_major",
            "feather_px_minor",
            "feather_px_major",
            "pixels_per_point",
            "alpha",
            "_pad0",
            "_pad1",
        ];
        assert_eq!(shader_fields, rust_fields);
    }

    #[test]
    fn a_tiles_version_tracks_its_arc_identity() {
        let a = Arc::new(vec![ContourPath {
            elevation_m: 1.0,
            points: Vec::new(),
        }]);
        let b = Arc::new(vec![ContourPath {
            elevation_m: 1.0,
            points: Vec::new(),
        }]);
        assert_eq!(version_key(&a, 7), version_key(&a.clone(), 7));
        assert_ne!(version_key(&a, 7), version_key(&b, 7));
        // A palette change must invalidate baked colours.
        assert_ne!(version_key(&a, 7), version_key(&a, 8));
    }
}
