/// GPU-instanced contour-line renderer for the globe's vector layers.
///
/// Replaces the per-frame CPU projection of contour polylines (topo, SRTM,
/// coastlines, bathymetry isobaths, lunar/mars topo) — previously ~700k–1M
/// `project_geo_flat` calls per frame — with a vertex shader that applies the
/// identical yaw/pitch + perspective transform on the GPU. Contour geometry is
/// uploaded once per tile-set/palette change and re-rendered every frame at
/// the cost of a 64-byte uniform write and one instanced draw per layer.
///
/// # Architecture
/// *  [`ContourPassResources`] – created once at startup in `DashboardApp::new`
///    alongside `GlobePassResources`. Holds the pipeline, a dynamic-offset
///    uniform buffer with one 256-byte slot per [`ContourLayer`], and the
///    per-layer instance buffers.
/// *  [`instances_for`] – CPU-side cache mapping a contour `Arc` + palette to
///    a flat `Vec<SegmentInstance>` (one instance per polyline segment, unit
///    sphere endpoints precomputed). Rebuilt only when the tile `Arc` or the
///    theme palette changes, in parallel via rayon.
/// *  [`ContourCallback`] – constructed each frame per visible layer; its
///    `prepare` writes that layer's uniform slot and (re)uploads the instance
///    buffer if the cached version differs; its `paint` issues one
///    `draw(0..6, 0..segment_count)`.
use eframe::egui_wgpu;
use eframe::wgpu;
use eframe::wgpu::util::DeviceExt;
use rayon::prelude::*;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use super::contour_asset::ContourPath;
use super::globe_pass::color_to_linear;
use crate::model::{GeoPoint, GlobeViewState};

/// Identifies a contour layer. Each layer owns one uniform-buffer slot and one
/// instance buffer, so a layer must be drawn at most once per frame.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ContourLayer {
    GlobalTopo,
    SrtmGlobe,
    Coastlines,
    Bathymetry,
    LunarTopo,
    MarsTopo,
}

const LAYER_COUNT: usize = 6;
/// wgpu requires dynamic uniform offsets to be 256-aligned on most hardware.
const UNIFORM_STRIDE: u64 = 256;

impl ContourLayer {
    fn slot(self) -> u32 {
        match self {
            ContourLayer::GlobalTopo => 0,
            ContourLayer::SrtmGlobe => 1,
            ContourLayer::Coastlines => 2,
            ContourLayer::Bathymetry => 3,
            ContourLayer::LunarTopo => 4,
            ContourLayer::MarsTopo => 5,
        }
    }
}

// ── Uniform buffer layout (must match WGSL struct byte-for-byte) ─────────────
//
//  offset   size  field
//  ──────   ────  ───────────────────────────────────────────────
//    0       8   center            vec2<f32>  (logical points)
//    8       8   viewport_min      vec2<f32>  (physical pixels)
//   16       8   viewport_size     vec2<f32>  (physical pixels)
//   24       4   yaw_sin           f32
//   28       4   yaw_cos           f32
//   32       4   pitch_sin         f32
//   36       4   pitch_cos         f32
//   40       4   radius_focal      f32
//   44       4   camera_distance   f32
//   48       4   radius_offset     f32
//   52       4   alpha             f32
//   56       4   stroke_half_px    f32
//   60       4   feather_px        f32
//   64       4   pixels_per_point  f32
//   68      12   _pad0.._pad2      f32 ×3
//   80 bytes total
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct ContourUniforms {
    center: [f32; 2],
    viewport_min: [f32; 2],
    viewport_size: [f32; 2],
    yaw_sin: f32,
    yaw_cos: f32,
    pitch_sin: f32,
    pitch_cos: f32,
    radius_focal: f32,
    camera_distance: f32,
    radius_offset: f32,
    alpha: f32,
    stroke_half_px: f32,
    feather_px: f32,
    pixels_per_point: f32,
    _pad: [f32; 3],
}

// Mirror of the WGSL `Uniforms` struct in `contour_lines.wgsl` — same guard as
// `GlobeUniforms`: a layout drift becomes a compile error, not GPU garbage.
const _: () = assert!(std::mem::size_of::<ContourUniforms>() == 80);

// ── Segment instances ─────────────────────────────────────────────────────────

/// One polyline segment: unit-sphere endpoints plus a baked
/// premultiplied-linear colour. 28 bytes, `step_mode: Instance`.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct SegmentInstance {
    a: [f32; 3],
    b: [f32; 3],
    color: [u8; 4],
}

const _: () = assert!(std::mem::size_of::<SegmentInstance>() == 28);

#[inline]
fn unit_vec(p: GeoPoint) -> [f32; 3] {
    // Matches the sphere-space convention of `project_geo_flat`:
    // x toward lon 0, y toward the north pole, z toward lon 90°E.
    let lat = p.lat.to_radians();
    let lon = p.lon.to_radians();
    let lat_cos = lat.cos();
    [lat_cos * lon.cos(), lat.sin(), lat_cos * lon.sin()]
}

/// Quantize an egui colour (premultiplied sRGB) to premultiplied-linear u8s,
/// the format the instance buffer stores and the shader consumes directly.
///
/// The alpha channel goes through the same sRGB→linear curve as the colour
/// channels: egui blends translucent strokes in gamma space, this pass blends
/// in linear space, and over the dark globe background gamma blending is
/// equivalent to linear blending with alpha raised to ~2.2. Without this the
/// GPU lines render dramatically brighter than the CPU path they replaced.
fn linear_u8(c: egui::Color32) -> [u8; 4] {
    let mut l = color_to_linear(c);
    l[3] = (c.a() as f32 / 255.0).powf(2.2);
    l.map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8)
}

#[inline]
fn contour_stroke_half_px(stroke_width_px: f32) -> f32 {
    stroke_width_px * 0.5
}

/// Keep the anti-alias fringe narrow enough that dense global contours do not
/// visually merge into bands. A half-pixel fringe is sufficient for the
/// one-pixel minimum; wider strokes receive a little more edge coverage while
/// never exceeding the old one-pixel fringe.
#[inline]
pub(crate) fn contour_feather_px(stroke_width_px: f32) -> f32 {
    const MIN_FEATHER_PX: f32 = 0.5;
    const MAX_FEATHER_PX: f32 = 1.0;
    const FEATHER_FRACTION: f32 = 0.1;

    let width = if stroke_width_px.is_finite() && stroke_width_px > 0.0 {
        stroke_width_px
    } else {
        1.0
    };
    (width * FEATHER_FRACTION).clamp(MIN_FEATHER_PX, MAX_FEATHER_PX)
}

/// Cheap identity for a contour set + palette combination. The `Arc` pointer
/// stands in for the tile contents (tile loads always allocate a fresh vec);
/// the palette key folds in whatever colours the layer bakes per contour, so a
/// theme switch triggers a rebuild.
fn version_key(contours: &Arc<Vec<ContourPath>>, palette_key: u64) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (Arc::as_ptr(contours) as *const () as usize).hash(&mut h);
    palette_key.hash(&mut h);
    h.finish()
}

/// Pack two theme colours into a palette key for [`instances_for`].
pub fn palette_key(major: egui::Color32, minor: egui::Color32) -> u64 {
    ((u32::from_le_bytes(major.to_array()) as u64) << 32)
        | u32::from_le_bytes(minor.to_array()) as u64
}

#[derive(Default)]
struct LayerInstances {
    /// Last fully-built instance set, served to callers even while stale.
    current: Option<(u64, Arc<Vec<SegmentInstance>>)>,
    /// Version a background build is currently producing, if any.
    building: Option<u64>,
}

/// Complete a single-flight build. A failed build deliberately retains the
/// previous instance set, but always releases the marker so a later repaint
/// can retry the current contour version.
fn complete_instance_build(
    entry: &mut LayerInstances,
    version: u64,
    built: Option<Vec<SegmentInstance>>,
) -> bool {
    if entry.building != Some(version) {
        return false;
    }
    entry.building = None;
    if let Some(built) = built {
        entry.current = Some((version, Arc::new(built)));
    }
    true
}

/// Return the cached segment instances for `contours`. A tile-`Arc` or
/// palette change kicks off a **background** rebuild (a full rebuild is up to
/// ~1M segments — far too slow for the frame); meanwhile the previous
/// instance set keeps rendering, so tile loads never hitch the UI. Returns
/// `None` only before the very first build of a layer completes.
///
/// `color_fn` bakes the per-contour colour — zoom-dependent fades must go
/// through the uniform `alpha` instead, or every zoom step would force a
/// rebuild.
pub fn instances_for(
    layer: ContourLayer,
    contours: &Arc<Vec<ContourPath>>,
    palette: u64,
    ctx: &egui::Context,
    color_fn: impl Fn(&ContourPath) -> egui::Color32 + Send + Sync + 'static,
) -> Option<(u64, Arc<Vec<SegmentInstance>>)> {
    let version = version_key(contours, palette);

    static CACHE: OnceLock<Mutex<HashMap<ContourLayer, LayerInstances>>> = OnceLock::new();
    let cache: &'static Mutex<HashMap<ContourLayer, LayerInstances>> =
        CACHE.get_or_init(|| Mutex::new(HashMap::new()));

    let mut guard = cache.lock().unwrap();
    let entry = guard.entry(layer).or_default();

    if let Some((v, instances)) = &entry.current {
        if *v == version {
            return Some((*v, instances.clone()));
        }
    }

    if entry.building.is_none() {
        // Keep one full instance rebuild in flight per layer. Globe tile loads
        // can arrive while an earlier build runs; starting another full Rayon
        // flatten for every intermediate version causes a thread/work storm.
        // The next repaint after this build commits observes the latest version
        // and schedules it if needed.
        entry.building = Some(version);
        let contours = contours.clone();
        let ctx = ctx.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("contour-instance-build".into())
            .spawn(move || {
                let built = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    puffin::profile_scope!("contour_instances_rebuild");
                    contours
                        .par_iter()
                        .flat_map_iter(|contour| {
                            let color = linear_u8(color_fn(contour));
                            contour.points.windows(2).map(move |pair| SegmentInstance {
                                a: unit_vec(pair[0]),
                                b: unit_vec(pair[1]),
                                color,
                            })
                        })
                        .collect()
                })) {
                    Ok(built) => Some(built),
                    Err(_) => {
                        eprintln!("[1kEE] contour instance worker panicked; retrying");
                        None
                    }
                };
                let mut guard = cache.lock().unwrap();
                let entry = guard.entry(layer).or_default();
                if complete_instance_build(entry, version, built) {
                    drop(guard);
                    // Wake the UI so a fresh result, or a recovered failure,
                    // is observed on the next frame.
                    ctx.request_repaint();
                }
            })
        {
            entry.building = None;
            eprintln!("[1kEE] failed to spawn contour instance worker: {error}");
        }
    }

    // Serve the previous (stale) set while the rebuild runs in the background.
    entry.current.clone()
}

// ── Persistent GPU resources ──────────────────────────────────────────────────

pub struct ContourPassResources {
    pipeline: wgpu::RenderPipeline,
    uniform_buf: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    layers: HashMap<ContourLayer, LayerGpu>,
}

struct LayerGpu {
    version: u64,
    instances: wgpu::Buffer,
    count: u32,
}

impl ContourPassResources {
    /// Create the render pipeline and the slotted uniform buffer.
    /// Call once from `DashboardApp::new`, next to `GlobePassResources::new`.
    pub fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("contour_pass_shader"),
            source: wgpu::ShaderSource::Wgsl(CONTOUR_WGSL.into()),
        });

        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("contour_uniforms"),
            size: UNIFORM_STRIDE * LAYER_COUNT as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("contour_bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(
                        std::mem::size_of::<ContourUniforms>() as u64
                    ),
                },
                count: None,
            }],
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("contour_bg"),
            layout: &bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &uniform_buf,
                    offset: 0,
                    size: wgpu::BufferSize::new(std::mem::size_of::<ContourUniforms>() as u64),
                }),
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("contour_pl"),
            bind_group_layouts: &[&bgl],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("contour_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<SegmentInstance>() as u64,
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
            layers: HashMap::new(),
        }
    }
}

// ── Per-frame callback ────────────────────────────────────────────────────────

pub struct ContourCallback {
    layer: ContourLayer,
    version: u64,
    instances: Arc<Vec<SegmentInstance>>,
    /// `screen_size` is filled in during `prepare` from the screen descriptor.
    uniforms: ContourUniforms,
}

impl ContourCallback {
    /// Build a callback for one layer. `radius_offset` is the constant
    /// altitude offset previously passed to `draw_geo_path`; `alpha` is the
    /// layer fade multiplier (zoom crossfades × the 0.92 stroke dimming the
    /// CPU path applied). `stroke_width_px` changes only this callback's uniform,
    /// so adjusting it never rebuilds contour instances.
    pub fn new(
        layer: ContourLayer,
        version: u64,
        instances: Arc<Vec<SegmentInstance>>,
        layout: &super::globe_scene::GlobeLayout,
        view: &GlobeViewState,
        radius_offset: f32,
        alpha: f32,
        stroke_width_px: f32,
        pixels_per_point: f32,
    ) -> Self {
        Self {
            layer,
            version,
            instances,
            uniforms: ContourUniforms {
                center: [layout.center.x, layout.center.y],
                // Filled in by `into_paint_callback` once the rect is known.
                viewport_min: [0.0, 0.0],
                viewport_size: [1.0, 1.0],
                yaw_sin: view.yaw.sin(),
                yaw_cos: view.yaw.cos(),
                pitch_sin: view.pitch.sin(),
                pitch_cos: view.pitch.cos(),
                radius_focal: layout.radius * layout.focal_length,
                camera_distance: layout.camera_distance,
                radius_offset,
                // Same gamma-space→linear-space correction as `linear_u8`:
                // the CPU path applied this fade via `gamma_multiply`.
                alpha: alpha.powf(2.2),
                // The UI-selected width is already in physical pixels.
                stroke_half_px: contour_stroke_half_px(stroke_width_px),
                feather_px: contour_feather_px(stroke_width_px),
                pixels_per_point,
                _pad: [0.0; 3],
            },
        }
    }

    /// egui-wgpu executes paint callbacks with the GPU viewport set to `rect`
    /// (in physical pixels), so the NDC transform must be relative to it.
    pub fn into_paint_callback(mut self, rect: egui::Rect) -> egui::PaintCallback {
        let ppp = self.uniforms.pixels_per_point;
        self.uniforms.viewport_min = [rect.min.x * ppp, rect.min.y * ppp];
        self.uniforms.viewport_size = [
            (rect.width() * ppp).max(1.0),
            (rect.height() * ppp).max(1.0),
        ];
        egui_wgpu::Callback::new_paint_callback(rect, self)
    }
}

impl egui_wgpu::CallbackTrait for ContourCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen_descriptor: &egui_wgpu::ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(res) = resources.get_mut::<ContourPassResources>() else {
            return Vec::new();
        };

        queue.write_buffer(
            &res.uniform_buf,
            self.layer.slot() as u64 * UNIFORM_STRIDE,
            bytemuck::bytes_of(&self.uniforms),
        );

        let stale = res
            .layers
            .get(&self.layer)
            .map(|gpu| gpu.version != self.version)
            .unwrap_or(true);
        if stale {
            puffin::profile_scope!("contour_instances_upload");
            res.layers.insert(
                self.layer,
                LayerGpu {
                    version: self.version,
                    instances: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("contour_instances"),
                        contents: bytemuck::cast_slice(&self.instances),
                        usage: wgpu::BufferUsages::VERTEX,
                    }),
                    count: self.instances.len() as u32,
                },
            );
        }
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        resources: &egui_wgpu::CallbackResources,
    ) {
        let Some(res) = resources.get::<ContourPassResources>() else {
            return;
        };
        let Some(gpu) = res.layers.get(&self.layer) else {
            return;
        };
        if gpu.count == 0 {
            return;
        }
        render_pass.set_pipeline(&res.pipeline);
        render_pass.set_bind_group(
            0,
            &res.bind_group,
            &[self.layer.slot() * UNIFORM_STRIDE as u32],
        );
        render_pass.set_vertex_buffer(0, gpu.instances.slice(..));
        render_pass.draw(0..6, 0..gpu.count);
    }
}

// ── WGSL shader ───────────────────────────────────────────────────────────────
//
// Source lives in `contour_lines.wgsl`. Its `Uniforms` struct must stay
// byte-for-byte in sync with `ContourUniforms` above.
const CONTOUR_WGSL: &str = include_str!("contour_lines.wgsl");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn physical_stroke_width_maps_directly_to_the_gpu_half_width() {
        assert_eq!(contour_stroke_half_px(1.0), 0.5);
        assert_eq!(contour_stroke_half_px(2.3), 1.15);
    }

    #[test]
    fn contour_feather_stays_narrow_at_small_widths() {
        assert_eq!(contour_feather_px(1.0), 0.5);
        assert_eq!(contour_feather_px(5.0), 0.5);
        assert_eq!(contour_feather_px(16.0), 1.0);
        assert_eq!(contour_feather_px(f32::NAN), 0.5);
    }

    #[test]
    fn failed_instance_build_releases_the_single_flight_gate() {
        let mut entry = LayerInstances {
            current: Some((3, Arc::new(Vec::new()))),
            building: Some(7),
        };

        assert!(complete_instance_build(&mut entry, 7, None));
        assert_eq!(entry.building, None);
        assert_eq!(entry.current.as_ref().map(|(version, _)| *version), Some(3));
        assert!(!complete_instance_build(&mut entry, 7, Some(Vec::new())));
    }
}
