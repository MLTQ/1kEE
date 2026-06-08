/// GPU-accelerated globe backdrop rendered via a wgpu PaintCallback.
///
/// Replaces the CPU-side `draw_backdrop` (flat circle fill) and `draw_graticule`
/// (lat/lon polylines) with a single fullscreen-triangle fragment shader that
/// performs per-pixel ray-sphere intersection, terrain-elevation shading, and
/// anti-aliased graticule lines — all at essentially zero CPU cost.
///
/// # Architecture
/// *  [`GlobePassResources`] – created once at startup in `DashboardApp::new` and
///    stored in `egui_wgpu::CallbackResources`.  Contains the wgpu `RenderPipeline`,
///    bind-group, and the uniform `Buffer`.
/// *  [`GlobeCallback`] – constructed each frame by `globe_scene::paint` and
///    submitted as an `egui::PaintCallback`.  Its `prepare` step uploads the
///    per-frame uniforms; its `paint` step issues a 3-vertex draw call.
use eframe::egui_wgpu;
use eframe::wgpu;

// ── Uniform buffer layout (must match WGSL struct byte-for-byte) ─────────────
//
//  offset   size  field
//  ──────   ────  ─────────────────────────────────────────────────────────────
//    0       8   center              vec2<f32>
//    8       4   radius              f32
//   12       4   focal_length        f32
//   16       4   camera_distance     f32
//   20       4   yaw                 f32
//   24       4   pitch               f32
//   28       4   pixels_per_point    f32
//   32      16   ocean_col           vec4<f32>   (linear RGBA)
//   48      16   land_col            vec4<f32>
//   64      16   mount_col           vec4<f32>
//   80      16   grid_col            vec4<f32>
//   96      16   hot_col             vec4<f32>
//  112       4   show_graticule      u32
//  116       4   active_body         u32
//  120       8   _pad                [u32; 2]
//  128 bytes total
//
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GlobeUniforms {
    // ── Camera / projection ────────────────────────────────────────────────
    pub center: [f32; 2],
    pub radius: f32,
    pub focal_length: f32,
    pub camera_distance: f32,
    pub yaw: f32,
    pub pitch: f32,
    pub pixels_per_point: f32,
    // ── Theme colours (sRGB → linear converted in Rust) ────────────────────
    pub ocean_col: [f32; 4],
    pub land_col: [f32; 4],
    pub mount_col: [f32; 4],
    pub grid_col: [f32; 4],
    pub hot_col: [f32; 4],
    // ── Flags ──────────────────────────────────────────────────────────────
    pub show_graticule: u32,
    pub active_body: u32,
    pub _pad: [u32; 2],
}

// The WGSL `Uniforms` struct in `globe.wgsl` is hand-aligned to this exact 128-byte
// layout (see the offset table above). If a field is added/removed/reordered here
// without mirroring it in the shader, the GPU reads garbage. This guard turns that
// silent corruption into a compile error; update both sides when it fires.
const _: () = assert!(std::mem::size_of::<GlobeUniforms>() == 128);

/// Convert an egui `Color32` (sRGB, [0, 255]) to linear-float `[f32; 4]`.
///
/// The wgpu surface is `*Srgb`-formatted, so the hardware automatically applies
/// sRGB encoding to whatever we write in the fragment shader — meaning we must
/// supply linear values.
pub fn color_to_linear(c: egui::Color32) -> [f32; 4] {
    [
        srgb_byte_to_linear(c.r()),
        srgb_byte_to_linear(c.g()),
        srgb_byte_to_linear(c.b()),
        c.a() as f32 / 255.0, // alpha is already linear
    ]
}

fn srgb_byte_to_linear(v: u8) -> f32 {
    let s = v as f32 / 255.0;
    if s <= 0.04045 {
        s / 12.92
    } else {
        ((s + 0.055) / 1.055).powf(2.4)
    }
}

// ── Persistent GPU resources ──────────────────────────────────────────────────

pub struct GlobePassResources {
    pipeline: wgpu::RenderPipeline,
    uniform_buf: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

impl GlobePassResources {
    /// Create the render pipeline and allocate the uniform buffer.
    /// Call this once from `DashboardApp::new` using `cc.wgpu_render_state`.
    pub fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("globe_pass_shader"),
            source: wgpu::ShaderSource::Wgsl(GLOBE_WGSL.into()),
        });

        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globe_uniforms"),
            size: std::mem::size_of::<GlobeUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globe_bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globe_bg"),
            layout: &bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buf.as_entire_binding(),
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("globe_pl"),
            bind_group_layouts: &[&bgl],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("globe_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
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
        }
    }
}

// ── Per-frame callback ────────────────────────────────────────────────────────

pub struct GlobeCallback {
    uniforms: GlobeUniforms,
}

impl GlobeCallback {
    /// Construct a paint callback from the current globe layout and view state.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        center: egui::Pos2,
        radius: f32,
        focal_length: f32,
        camera_distance: f32,
        yaw: f32,
        pitch: f32,
        pixels_per_point: f32,
        show_graticule: bool,
        active_body: crate::model::ActiveBody,
        ocean_col: egui::Color32,
        land_col: egui::Color32,
        mount_col: egui::Color32,
        grid_col: egui::Color32,
        hot_col: egui::Color32,
    ) -> Self {
        Self {
            uniforms: GlobeUniforms {
                center: [center.x, center.y],
                radius,
                focal_length,
                camera_distance,
                yaw,
                pitch,
                pixels_per_point,
                ocean_col: color_to_linear(ocean_col),
                land_col: color_to_linear(land_col),
                mount_col: color_to_linear(mount_col),
                grid_col: color_to_linear(grid_col),
                hot_col: color_to_linear(hot_col),
                show_graticule: show_graticule as u32,
                active_body: match active_body {
                    crate::model::ActiveBody::Earth => 0,
                    crate::model::ActiveBody::Moon => 1,
                    crate::model::ActiveBody::Mars => 2,
                },
                _pad: [0; 2],
            },
        }
    }

    /// Wrap into an [`egui::PaintCallback`] covering `rect`.
    pub fn into_paint_callback(self, rect: egui::Rect) -> egui::PaintCallback {
        egui_wgpu::Callback::new_paint_callback(rect, self)
    }
}

impl egui_wgpu::CallbackTrait for GlobeCallback {
    fn prepare(
        &self,
        _device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen_descriptor: &egui_wgpu::ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if let Some(res) = resources.get::<GlobePassResources>() {
            queue.write_buffer(&res.uniform_buf, 0, bytemuck::bytes_of(&self.uniforms));
        }
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        resources: &egui_wgpu::CallbackResources,
    ) {
        if let Some(res) = resources.get::<GlobePassResources>() {
            render_pass.set_pipeline(&res.pipeline);
            render_pass.set_bind_group(0, &res.bind_group, &[]);
            render_pass.draw(0..3, 0..1);
        }
    }
}

// ── WGSL shader ───────────────────────────────────────────────────────────────
//
// Source lives in `globe.wgsl` (extracted from this file so editors give it WGSL
// syntax highlighting and validation). Its `Uniforms` struct must stay
// byte-for-byte in sync with `GlobeUniforms` above — see the offset table and the
// `size_of` assertion near the struct definition.
const GLOBE_WGSL: &str = include_str!("globe.wgsl");
