/// GPU-accelerated terrain surface for the local/regional oblique view.
///
/// Renders a smooth, Lambertian-shaded terrain mesh under the CPU-drawn
/// contour lines, roads, and markers.  The vertex shader replicates
/// `project_local` in WGSL; the fragment shader computes surface normals
/// from the heightmap and blends through an elevation colour ramp.
///
/// # Architecture
/// * [`LocalTerrainPassResources`] – created once at startup in `DashboardApp::new`
///   and stored in `egui_wgpu::CallbackResources`.  Contains the render pipeline,
///   a 128×128 `Rgba8Unorm` heightmap texture, a pre-built index buffer for a
///   64×64 quad grid, and the uniform buffer.
/// * [`LocalTerrainCallback`] – constructed each frame by `local_terrain_scene::paint`.
///   Its `prepare` step rebuilds/uploads the heightmap when the viewport changes and
///   writes the per-frame uniforms; its `paint` step issues one indexed draw call.
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, atomic::{AtomicBool, Ordering}};

use eframe::egui_wgpu;
use eframe::wgpu;
use wgpu::util::DeviceExt as _;

use crate::model::GeoPoint;

use super::terrain_raster;

// ── Non-blocking heightmap build ──────────────────────────────────────────────
//
// `build_heightmap` samples 16 384 elevation points, each of which acquires
// the global SRTM tile-cache mutex.  Calling it from `prepare()` (the wgpu
// render thread) would block the render thread whenever a background contour
// loading thread is simultaneously holding that mutex.  The resulting frame
// drops look like a strobe.
//
// Instead we kick off a background thread the first time (or whenever the
// viewport key changes), immediately return from `prepare()`, and on the NEXT
// `prepare()` call check whether the result is ready.  The texture shows the
// previous viewport's heightmap (or flat zeros on the very first frame) until
// the background build completes — which is imperceptible at render speed.

struct PendingHeightmap {
    key: HeightmapKey,
    data: Vec<u8>,
}

fn pending_hmap_slot() -> &'static Mutex<Option<PendingHeightmap>> {
    static SLOT: OnceLock<Mutex<Option<PendingHeightmap>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

static BUILDING: AtomicBool = AtomicBool::new(false);

/// Tracks the HeightmapKey that is currently uploaded and valid on the GPU.
/// Stored as a flat tuple (lat_q, lon_q, extent_q, root_hash) to avoid leaking
/// the private `HeightmapKey` type.
fn ready_key_slot() -> &'static Mutex<Option<(i32, i32, i32, u64)>> {
    static SLOT: OnceLock<Mutex<Option<(i32, i32, i32, u64)>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

/// Returns `true` if the GPU heightmap for the given viewport is already
/// uploaded and ready to render.  When `false` the terrain callback should
/// be skipped so the loading animation and contour lines remain visible.
pub fn is_heightmap_ready(center: GeoPoint, half_extent_deg: f32, selected_root: Option<&std::path::Path>) -> bool {
    let key = HeightmapKey::new(center, half_extent_deg, selected_root);
    let tuple = (key.lat_q, key.lon_q, key.extent_q, key.root_hash);
    ready_key_slot().lock().map(|g| *g == Some(tuple)).unwrap_or(false)
}

// ── Constants ──────────────────────────────────────────────────────────────────

/// Side length of the heightmap texture (pixels).
/// 256×256 gives one sample per ~400m at typical zoom, matching SRTM contour detail.
const HMAP_SIZE: u32 = 256;

/// Number of quads per side of the terrain grid.
/// 128×128 = 16 384 triangles, still trivial for GPU but visually matches heightmap resolution.
const GRID_N: u32 = 128;

/// Total index count: GRID_N × GRID_N quads × 2 triangles × 3 indices.
const INDEX_COUNT: u32 = GRID_N * GRID_N * 6;

/// Elevation encoding range.  Values outside this range are clamped.
const ELEV_MIN_M: f32 = -2_000.0;
const ELEV_RANGE_M: f32 = 12_000.0; // −2 km … +10 km

#[allow(dead_code)]
const BASE_VERT_EXAG: f32 = 2.1;

// ── Uniform buffer layout (must match WGSL struct byte-for-byte) ──────────────
//
//  offset   size   field
//  ──────   ────   ─────────────────────────────────────────────────────────────
//    0        4    focus_lat
//    4        4    focus_lon
//    8        4    half_extent_deg
//   12        4    km_per_deg_lon   (= 111.32 * cos(focus_lat))
//   16        4    extent_x_km
//   20        4    extent_y_km
//   24        4    reference_span_km
//   28        4    _pad0
//   32        4    focus_center_x   (screen-space px)
//   36        4    focus_center_y
//   40        4    horizontal_scale
//   44        4    layout_height
//   48        4    yaw_cos
//   52        4    yaw_sin
//   56        4    pitch_cos
//   60        4    pitch_sin
//   64        4    layer_spread
//   68        4    alpha            (overall mesh opacity)
//   72        4    screen_width     (physical px)
//   76        4    screen_height
//   80        4    elev_min_m       (heightmap decode)
//   84        4    elev_range_m
//   88        4    _pad1
//   92        4    _pad2
//   96       16    backdrop_col     vec4<f32> linear RGBA
//  112       16    low_col
//  128       16    high_col
//  144       16    peak_col
//  160 bytes total
//
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct LocalTerrainUniforms {
    focus_lat:         f32,
    focus_lon:         f32,
    half_extent_deg:   f32,
    km_per_deg_lon:    f32,

    extent_x_km:       f32,
    extent_y_km:       f32,
    reference_span_km: f32,
    _pad0:             f32,

    focus_center_x:    f32,
    focus_center_y:    f32,
    horizontal_scale:  f32,
    layout_height:     f32,

    yaw_cos:           f32,
    yaw_sin:           f32,
    pitch_cos:         f32,
    pitch_sin:         f32,

    layer_spread:      f32,
    alpha:             f32,
    screen_width:      f32,
    screen_height:     f32,

    elev_min_m:        f32,
    elev_range_m:      f32,
    _pad1:             f32,
    _pad2:             f32,

    backdrop_col:      [f32; 4],
    low_col:           [f32; 4],
    high_col:          [f32; 4],
    peak_col:          [f32; 4],
}

fn color_to_linear(c: egui::Color32) -> [f32; 4] {
    fn s(v: u8) -> f32 {
        let f = v as f32 / 255.0;
        if f <= 0.04045 { f / 12.92 } else { ((f + 0.055) / 1.055).powf(2.4) }
    }
    [s(c.r()), s(c.g()), s(c.b()), c.a() as f32 / 255.0]
}

// ── Cache key for heightmap rebuilds ─────────────────────────────────────────

#[derive(PartialEq, Clone)]
struct HeightmapKey {
    /// Quantized to 4 decimal places (~11 m) to avoid per-frame rebuilds.
    lat_q:     i32,
    lon_q:     i32,
    extent_q:  i32,
    root_hash: u64,
}

impl HeightmapKey {
    fn new(center: GeoPoint, half_extent_deg: f32, selected_root: Option<&Path>) -> Self {
        let root_hash = selected_root
            .map(|p| {
                use std::hash::{Hash, Hasher};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                p.hash(&mut h);
                h.finish()
            })
            .unwrap_or(0);
        Self {
            lat_q:     (center.lat * 1_000.0).round() as i32,
            lon_q:     (center.lon * 1_000.0).round() as i32,
            extent_q:  (half_extent_deg * 10_000.0).round() as i32,
            root_hash,
        }
    }
}

// ── Persistent GPU resources ──────────────────────────────────────────────────

pub struct LocalTerrainPassResources {
    pipeline:      wgpu::RenderPipeline,
    uniform_buf:   wgpu::Buffer,
    heightmap_tex: wgpu::Texture,
    bind_group:    wgpu::BindGroup,
    index_buf:     wgpu::Buffer,
    cached_key:    Option<HeightmapKey>,
}

impl LocalTerrainPassResources {
    pub fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label:  Some("local_terrain_shader"),
            source: wgpu::ShaderSource::Wgsl(LOCAL_TERRAIN_WGSL.into()),
        });

        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label:              Some("local_terrain_uniforms"),
            size:               std::mem::size_of::<LocalTerrainUniforms>() as u64,
            usage:              wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let heightmap_tex = device.create_texture(&wgpu::TextureDescriptor {
            label:           Some("local_terrain_heightmap"),
            size:            wgpu::Extent3d { width: HMAP_SIZE, height: HMAP_SIZE, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count:    1,
            dimension:       wgpu::TextureDimension::D2,
            format:          wgpu::TextureFormat::Rgba8Unorm,
            usage:           wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats:    &[],
        });
        let heightmap_view = heightmap_tex.create_view(&wgpu::TextureViewDescriptor::default());

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label:             Some("local_terrain_sampler"),
            address_mode_u:    wgpu::AddressMode::ClampToEdge,
            address_mode_v:    wgpu::AddressMode::ClampToEdge,
            mag_filter:        wgpu::FilterMode::Linear,
            min_filter:        wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label:   Some("local_terrain_bgl"),
            entries: &[
                // binding 0: uniform buffer
                wgpu::BindGroupLayoutEntry {
                    binding:    0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty:                 wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size:   None,
                    },
                    count: None,
                },
                // binding 1: heightmap texture
                wgpu::BindGroupLayoutEntry {
                    binding:    1,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type:    wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled:   false,
                    },
                    count: None,
                },
                // binding 2: sampler
                wgpu::BindGroupLayoutEntry {
                    binding:    2,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label:   Some("local_terrain_bg"),
            layout:  &bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: uniform_buf.as_entire_binding() },
                wgpu::BindGroupEntry {
                    binding:  1,
                    resource: wgpu::BindingResource::TextureView(&heightmap_view),
                },
                wgpu::BindGroupEntry {
                    binding:  2,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label:                Some("local_terrain_pl"),
            bind_group_layouts:   &[&bgl],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label:  Some("local_terrain_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module:               &shader,
                entry_point:          Some("vs_main"),
                compilation_options:  Default::default(),
                buffers:              &[],  // procedural vertices from vertex_index
            },
            fragment: Some(wgpu::FragmentState {
                module:              &shader,
                entry_point:         Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format:     target_format,
                    blend:      Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology:           wgpu::PrimitiveTopology::TriangleList,
                front_face:         wgpu::FrontFace::Ccw,
                cull_mode:          None, // terrain can be seen from either side during tilt
                ..Default::default()
            },
            depth_stencil: None,
            multisample:   wgpu::MultisampleState::default(),
            multiview:     None,
            cache:         None,
        });

        // Pre-build the index buffer for a GRID_N×GRID_N quad mesh.
        let indices = build_grid_indices(GRID_N);
        let index_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label:    Some("local_terrain_indices"),
            contents: bytemuck::cast_slice(&indices),
            usage:    wgpu::BufferUsages::INDEX,
        });

        Self { pipeline, uniform_buf, heightmap_tex, bind_group, index_buf, cached_key: None }
    }
}

fn build_grid_indices(n: u32) -> Vec<u16> {
    let mut idx = Vec::with_capacity((n * n * 6) as usize);
    for row in 0..n {
        for col in 0..n {
            let tl = (row       * (n + 1) + col    ) as u16;
            let tr = (row       * (n + 1) + col + 1) as u16;
            let bl = ((row + 1) * (n + 1) + col    ) as u16;
            let br = ((row + 1) * (n + 1) + col + 1) as u16;
            idx.extend_from_slice(&[tl, tr, bl, tr, br, bl]);
        }
    }
    idx
}

/// Build a HMAP_SIZE×HMAP_SIZE Rgba8Unorm heightmap from SRTM / GEBCO elevation data.
/// R channel = normalized elevation; G,B,A unused (set to 0/0/255).
fn build_heightmap(
    center: GeoPoint,
    half_extent_deg: f32,
    selected_root: Option<&Path>,
) -> Vec<u8> {
    let n = HMAP_SIZE as usize;
    let mut data = vec![0u8; n * n * 4]; // Rgba8Unorm: 4 bytes per texel

    for row in 0..n {
        for col in 0..n {
            let u = col as f32 / (n - 1) as f32;
            let v = row as f32 / (n - 1) as f32;
            // v=0 → north (max lat), v=1 → south (min lat)
            let lat = center.lat + (0.5 - v) * 2.0 * half_extent_deg;
            let lon = center.lon + (u - 0.5) * 2.0 * half_extent_deg;
            let elev = terrain_raster::sample_elevation_m(selected_root, GeoPoint { lat, lon })
                .unwrap_or(0.0);
            let normalized = ((elev - ELEV_MIN_M) / ELEV_RANGE_M).clamp(0.0, 1.0);
            let encoded = (normalized * 255.0).round() as u8;
            let base = (row * n + col) * 4;
            data[base]     = encoded; // R = elevation
            data[base + 3] = 255;     // A = opaque (unused but keeps format valid)
        }
    }
    data
}

/// Upload a completed heightmap byte buffer to the GPU texture.
fn upload_heightmap(queue: &wgpu::Queue, tex: &wgpu::Texture, data: &[u8]) {
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture:   tex,
            mip_level: 0,
            origin:    wgpu::Origin3d::ZERO,
            aspect:    wgpu::TextureAspect::All,
        },
        data,
        wgpu::TexelCopyBufferLayout {
            offset:         0,
            bytes_per_row:  Some(HMAP_SIZE * 4),
            rows_per_image: Some(HMAP_SIZE),
        },
        wgpu::Extent3d { width: HMAP_SIZE, height: HMAP_SIZE, depth_or_array_layers: 1 },
    );
}

// ── Per-frame callback ────────────────────────────────────────────────────────

/// Parameters for one frame's terrain draw.  Construct via [`LocalTerrainCallback::new`]
/// and submit with [`into_paint_callback`].
pub struct LocalTerrainCallback {
    center:         GeoPoint,
    half_extent_deg: f32,
    selected_root:  Option<PathBuf>,
    uniforms:       LocalTerrainUniforms,
}

/// Layout parameters mirroring `LocalLayout` from `local_terrain_scene`.
pub struct LocalTerrainLayout {
    pub focus_center:    egui::Pos2,
    pub horizontal_scale: f32,
    pub height:          f32,
}

impl LocalTerrainCallback {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        center:          GeoPoint,
        half_extent_deg: f32,
        layout:          &LocalTerrainLayout,
        view_yaw:        f32,
        view_pitch:      f32,
        layer_spread:    f32,
        alpha:           f32,
        selected_root:   Option<&Path>,
        backdrop_col:    egui::Color32,
        low_col:         egui::Color32,
        high_col:        egui::Color32,
        peak_col:        egui::Color32,
    ) -> Self {
        let km_per_deg_lat = 111.32_f32;
        let km_per_deg_lon = km_per_deg_lat * center.lat.to_radians().cos().abs().max(0.2);
        let extent_x_km    = (half_extent_deg * km_per_deg_lon).max(1.0);
        let extent_y_km    = (half_extent_deg * km_per_deg_lat).max(1.0);
        let reference_span_km = (extent_x_km + extent_y_km) * 0.5;

        Self {
            center,
            half_extent_deg,
            selected_root: selected_root.map(PathBuf::from),
            uniforms: LocalTerrainUniforms {
                focus_lat:         center.lat,
                focus_lon:         center.lon,
                half_extent_deg,
                km_per_deg_lon,
                extent_x_km,
                extent_y_km,
                reference_span_km,
                _pad0: 0.0,
                focus_center_x:    layout.focus_center.x,
                focus_center_y:    layout.focus_center.y,
                horizontal_scale:  layout.horizontal_scale,
                layout_height:     layout.height,
                yaw_cos:           view_yaw.cos(),
                yaw_sin:           view_yaw.sin(),
                pitch_cos:         view_pitch.cos(),
                pitch_sin:         view_pitch.sin(),
                layer_spread,
                alpha,
                screen_width:      0.0, // filled in prepare()
                screen_height:     0.0,
                elev_min_m:        ELEV_MIN_M,
                elev_range_m:      ELEV_RANGE_M,
                _pad1: 0.0,
                _pad2: 0.0,
                backdrop_col: color_to_linear(backdrop_col),
                low_col:      color_to_linear(low_col),
                high_col:     color_to_linear(high_col),
                peak_col:     color_to_linear(peak_col),
            },
        }
    }

    pub fn into_paint_callback(self, rect: egui::Rect) -> egui::PaintCallback {
        egui_wgpu::Callback::new_paint_callback(rect, self)
    }
}

impl egui_wgpu::CallbackTrait for LocalTerrainCallback {
    fn prepare(
        &self,
        _device: &wgpu::Device,
        queue: &wgpu::Queue,
        screen_descriptor: &egui_wgpu::ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(res) = resources.get_mut::<LocalTerrainPassResources>() else {
            return Vec::new();
        };

        // Patch screen dimensions (not known at callback construction time).
        let mut uniforms = self.uniforms;
        uniforms.screen_width  = screen_descriptor.size_in_pixels[0] as f32;
        uniforms.screen_height = screen_descriptor.size_in_pixels[1] as f32;
        queue.write_buffer(&res.uniform_buf, 0, bytemuck::bytes_of(&uniforms));

        // ── Non-blocking heightmap update ────────────────────────────────────
        let key = HeightmapKey::new(self.center, self.half_extent_deg, self.selected_root.as_deref());

        // 1. If a background build finished, upload the result now.
        if let Ok(mut slot) = pending_hmap_slot().lock() {
            if let Some(pending) = slot.take() {
                upload_heightmap(queue, &res.heightmap_tex, &pending.data);
                res.cached_key = Some(pending.key.clone());
                // Mark this key as ready so the scene layer can skip the
                // callback (and keep contours + loading animation visible)
                // until the texture is actually uploaded.
                if let Ok(mut rk) = ready_key_slot().lock() {
                    *rk = Some((pending.key.lat_q, pending.key.lon_q, pending.key.extent_q, pending.key.root_hash));
                }
            }
        }

        // If the already-cached key matches, ensure the ready slot reflects it
        // (covers the case where terrain was toggled off then back on).
        if res.cached_key.as_ref() == Some(&key) {
            if let Ok(mut rk) = ready_key_slot().lock() {
                if rk.is_none() {
                    *rk = Some((key.lat_q, key.lon_q, key.extent_q, key.root_hash));
                }
            }
        }

        // 2. If the uploaded key still doesn't match (viewport moved) and no
        //    build is in flight, kick off a new background build.  Never block.
        if res.cached_key.as_ref() != Some(&key)
            && !BUILDING.load(Ordering::Relaxed)
        {
            // Invalidate the ready key — we're about to build a new heightmap.
            if let Ok(mut rk) = ready_key_slot().lock() { *rk = None; }
            BUILDING.store(true, Ordering::Relaxed);
            let center       = self.center;
            let half_extent  = self.half_extent_deg;
            let root         = self.selected_root.clone();
            let key_clone    = key.clone();
            std::thread::spawn(move || {
                let data = build_heightmap(center, half_extent, root.as_deref());
                if let Ok(mut slot) = pending_hmap_slot().lock() {
                    *slot = Some(PendingHeightmap { key: key_clone, data });
                }
                BUILDING.store(false, Ordering::Relaxed);
                crate::app::request_repaint();
            });
        }

        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        resources: &egui_wgpu::CallbackResources,
    ) {
        let Some(res) = resources.get::<LocalTerrainPassResources>() else { return };

        // Skip the draw if the GPU texture doesn't yet hold this viewport's
        // heightmap.  prepare() still ran (uploading data if available and
        // kicking off background builds), so the terrain will appear as soon
        // as the background thread finishes — without showing a black quad in
        // the meantime or covering the loading animation beneath it.
        let key = HeightmapKey::new(self.center, self.half_extent_deg, self.selected_root.as_deref());
        if res.cached_key.as_ref() != Some(&key) { return; }

        render_pass.set_pipeline(&res.pipeline);
        render_pass.set_bind_group(0, &res.bind_group, &[]);
        render_pass.set_index_buffer(res.index_buf.slice(..), wgpu::IndexFormat::Uint16);
        // Vertex count: (GRID_N+1)² vertices, no vertex buffer — positions come
        // from vertex_index + heightmap sample in vs_main.
        render_pass.draw_indexed(0..INDEX_COUNT, 0, 0..1);
    }
}

// ── WGSL shader ───────────────────────────────────────────────────────────────

const LOCAL_TERRAIN_WGSL: &str = r#"

struct Uniforms {
    focus_lat:         f32,
    focus_lon:         f32,
    half_extent_deg:   f32,
    km_per_deg_lon:    f32,

    extent_x_km:       f32,
    extent_y_km:       f32,
    reference_span_km: f32,
    _pad0:             f32,

    focus_center_x:    f32,
    focus_center_y:    f32,
    horizontal_scale:  f32,
    layout_height:     f32,

    yaw_cos:    f32,
    yaw_sin:    f32,
    pitch_cos:  f32,
    pitch_sin:  f32,

    layer_spread:  f32,
    alpha:         f32,
    screen_width:  f32,
    screen_height: f32,

    elev_min_m:   f32,
    elev_range_m: f32,
    _pad1:        f32,
    _pad2:        f32,

    backdrop_col: vec4<f32>,
    low_col:      vec4<f32>,
    high_col:     vec4<f32>,
    peak_col:     vec4<f32>,
}

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var hmap: texture_2d<f32>;
@group(0) @binding(2) var hmap_samp: sampler;

const BASE_VERT_EXAG: f32 = 2.1;
const GRID_N: u32 = 128u;
const VERTS_PER_ROW: u32 = 129u;  // GRID_N + 1

struct VsOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) elevation_m: f32,
    @location(1) uv: vec2<f32>,
}

// ── project_local (mirrors the Rust function exactly) ──────────────────────
fn project_local_ndc(lat: f32, lon: f32, elevation_m: f32) -> vec2<f32> {
    let x_km = (lon - u.focus_lon) * u.km_per_deg_lon;
    let y_km = (lat - u.focus_lat) * 111.32;

    let x = x_km / u.extent_x_km;
    let y = y_km / u.extent_y_km;
    let z = (elevation_m / 1000.0) * BASE_VERT_EXAG / u.reference_span_km;

    let x_yaw = x * u.yaw_cos - y * u.yaw_sin;
    let y_yaw = x * u.yaw_sin + y * u.yaw_cos;

    let ground_y_pitch  = y_yaw * u.pitch_cos;
    let ground_z_pitch  = y_yaw * u.pitch_sin;
    let elev_y_offset   = z    * u.pitch_sin;
    let elev_z_offset   = z    * u.pitch_cos;

    let gps = u.layout_height * 0.55;                     // ground_pitch_scale
    // ground_depth_scale: subtle recede cue.  Must stay < gps/tan(max_pitch).
    // Max pitch = 1.55 rad → tan ≈ 48.  0.55/48 ≈ 0.0114, so 0.01 is safely below
    // the singularity while still giving a gentle depth-recession illusion.
    let gds = u.layout_height * 0.01;                     // ground_depth_scale
    let eps = u.layout_height * 0.55 * u.layer_spread;    // elev_pitch_scale
    let eds = u.layout_height * 0.24 * u.layer_spread;    // elev_depth_scale

    let px = u.focus_center_x + x_yaw * u.horizontal_scale;
    let py = u.focus_center_y
        - ground_y_pitch * gps
        + ground_z_pitch * gds
        - elev_y_offset  * eps
        - elev_z_offset  * eds;

    // Screen-space pixel → NDC  (wgpu: y=+1 is top)
    let ndc_x =  2.0 * px / u.screen_width  - 1.0;
    let ndc_y = -2.0 * py / u.screen_height + 1.0;
    return vec2<f32>(ndc_x, ndc_y);
}

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
    let row = vi / VERTS_PER_ROW;
    let col = vi % VERTS_PER_ROW;

    let uv = vec2<f32>(
        f32(col) / f32(GRID_N),
        f32(row) / f32(GRID_N),
    );

    // v=0 → north (high lat), v=1 → south (low lat)
    let lat = u.focus_lat + (0.5 - uv.y) * 2.0 * u.half_extent_deg;
    let lon = u.focus_lon + (uv.x - 0.5) * 2.0 * u.half_extent_deg;

    let encoded      = textureSampleLevel(hmap, hmap_samp, uv, 0.0).r;
    let elevation_m  = encoded * u.elev_range_m + u.elev_min_m;

    let ndc = project_local_ndc(lat, lon, elevation_m);

    var out: VsOut;
    out.clip_pos   = vec4<f32>(ndc.x, ndc.y, 0.5, 1.0);
    out.elevation_m = elevation_m;
    out.uv         = uv;
    return out;
}

// ── Elevation colour ramp ───────────────────────────────────────────────────
fn elevation_color(e: f32) -> vec3<f32> {
    if e <= 0.0 {
        // Below sea level: blend backdrop toward a slightly bluer shade
        let depth = clamp(-e / 1500.0, 0.0, 1.0);
        return mix(u.backdrop_col.rgb, u.backdrop_col.rgb * 0.5, depth);
    } else if e < 400.0 {
        let t = e / 400.0;
        return mix(u.backdrop_col.rgb * 1.15, u.low_col.rgb * 0.7, t);
    } else if e < 1500.0 {
        let t = (e - 400.0) / 1100.0;
        return mix(u.low_col.rgb * 0.7, u.high_col.rgb, t);
    } else {
        let t = clamp((e - 1500.0) / 2500.0, 0.0, 1.0);
        return mix(u.high_col.rgb, u.peak_col.rgb, t);
    }
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let uv = in.uv;

    // Surface normal via central finite differences on the heightmap.
    // Sample at ±1 texel in each direction.
    let ts = 1.0 / f32(HMAP_SIZE);
    let h_l = textureSampleLevel(hmap, hmap_samp, uv + vec2<f32>(-ts,  0.0), 0.0).r;
    let h_r = textureSampleLevel(hmap, hmap_samp, uv + vec2<f32>( ts,  0.0), 0.0).r;
    let h_d = textureSampleLevel(hmap, hmap_samp, uv + vec2<f32>( 0.0, -ts), 0.0).r;
    let h_u = textureSampleLevel(hmap, hmap_samp, uv + vec2<f32>( 0.0,  ts), 0.0).r;

    // Scale gradient by vertical-exaggeration factor so the normal responds
    // to the same relief that the mesh geometry shows.
    let dzdx = (h_r - h_l) * 6.0;
    let dzdy = (h_u - h_d) * 6.0;  // note: h_u is further north = lower v
    let normal = normalize(vec3<f32>(-dzdx, dzdy, 1.0));

    // Directional light from upper-left (standard cartographic illumination).
    let sun = normalize(vec3<f32>(0.55, 0.75, 1.0));
    let diffuse  = clamp(dot(normal, sun), 0.0, 1.0);
    let lighting = 0.38 + 0.62 * diffuse;

    let base_rgb = elevation_color(in.elevation_m);
    let lit_rgb  = base_rgb * lighting;

    // Pre-multiply alpha for correct blending with egui's compositor.
    let a = u.alpha;
    return vec4<f32>(lit_rgb * a, a);
}

const HMAP_SIZE: u32 = 128u;
"#;
