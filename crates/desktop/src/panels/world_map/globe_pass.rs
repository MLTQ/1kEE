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
//  116      12   _pad                [u32; 3]
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
    pub _pad: [u32; 3],
}

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
    if s <= 0.04045 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) }
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

        Self { pipeline, uniform_buf, bind_group }
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
                _pad: [0; 3],
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

const GLOBE_WGSL: &str = r#"
// ── Uniform struct ──────────────────────────────────────────────────────────
struct Uniforms {
    // Camera / projection  (bytes 0..31)
    center:          vec2<f32>,
    radius:          f32,
    focal_length:    f32,
    camera_distance: f32,
    yaw:             f32,
    pitch:           f32,
    pixels_per_point: f32,
    // Theme colours, linear RGBA  (bytes 32..111)
    ocean_col: vec4<f32>,
    land_col:  vec4<f32>,
    mount_col: vec4<f32>,
    grid_col:  vec4<f32>,
    hot_col:   vec4<f32>,
    // Flags  (bytes 112..127)
    show_graticule: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
}

@group(0) @binding(0) var<uniform> u: Uniforms;

const PI: f32 = 3.14159265358979;

// ── Vertex shader: fullscreen triangle ─────────────────────────────────────
@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    // Three vertices covering the entire NDC clip space.
    // Egui's scissor rect restricts rasterisation to the globe rect.
    var pos = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 3.0, -1.0),
        vec2<f32>(-1.0,  3.0),
    );
    return vec4<f32>(pos[vi], 0.0, 1.0);
}

// ── Terrain helpers ─────────────────────────────────────────────────────────

fn gauss(lat: f32, lon: f32, lat0: f32, lon0: f32, sigma: f32) -> f32 {
    let dlat = lat - lat0;
    var dlon = lon - lon0;
    if dlon > 180.0  { dlon = dlon - 360.0; }
    if dlon < -180.0 { dlon = dlon + 360.0; }
    return exp(-(dlat * dlat + dlon * dlon) / (2.0 * sigma * sigma));
}

/// Synthetic terrain elevation that replicates `terrain_field::elevation`.
/// Returns a value in [0, 1.6].
fn terrain_elev(lat_deg: f32, lon_deg: f32) -> f32 {
    let lat = lat_deg * (PI / 180.0);
    let lon = lon_deg * (PI / 180.0);

    let h = 0.42
        + 0.18 * sin(lat * 2.1) * cos(lon * 1.8)
        + 0.11 * sin(lat * 5.4 + lon * 1.3)
        + 0.08 * cos(lon * 3.7 - lat * 2.2)
        + 0.05 * sin(lon * 9.1 + lat * 7.3);

    var m = 0.0;
    m += gauss(lat_deg, lon_deg,  27.7,   86.9,  10.5) * 1.00; // Himalayas
    m += gauss(lat_deg, lon_deg, -32.7,  -70.1,  11.5) * 0.90; // Andes
    m += gauss(lat_deg, lon_deg,  46.5,   10.2,   6.5) * 0.55; // Alps
    m += gauss(lat_deg, lon_deg,  35.4,  138.7,   4.6) * 0.48; // Japan
    m += gauss(lat_deg, lon_deg,  61.0, -149.0,   7.2) * 0.52; // Alaska
    m += gauss(lat_deg, lon_deg, -43.6,  170.2,   5.6) * 0.42; // New Zealand

    return clamp(h + m, 0.0, 1.6);
}

/// Map normalised elevation [0..1] to a surface colour using the theme palette.
/// Blends ocean → land → mountain in linear space.
fn surface_color(e_norm: f32) -> vec3<f32> {
    let ocean = u.ocean_col.rgb;
    let land  = u.land_col.rgb;
    let mount = u.mount_col.rgb;

    if e_norm < 0.24 {
        // Deep ocean / below-sea-level
        return ocean;
    } else if e_norm < 0.46 {
        let t = (e_norm - 0.24) / 0.22;
        return mix(ocean, land, t);
    } else if e_norm < 0.72 {
        return land;
    } else {
        let t = clamp((e_norm - 0.72) / 0.28, 0.0, 1.0);
        return mix(land, mount, t);
    }
}

// ── Graticule ───────────────────────────────────────────────────────────────

/// α (0..1) for a graticule line centred at distance 0.
/// `half_w` is the half-width of the line in the same unit as `dist`.
fn line_alpha(dist: f32, half_w: f32) -> f32 {
    return 1.0 - smoothstep(0.0, half_w * 2.0, dist);
}

/// Distance to the nearest multiple of `step` (wrapping-aware for longitude).
fn deg_dist_to_step(val: f32, step: f32) -> f32 {
    let n = val / step;
    return abs(n - round(n)) * step;
}

/// Composite the graticule contribution into `base_rgb`.
fn apply_graticule(
    base: vec3<f32>,
    lat_deg: f32,
    lon_deg: f32,
    half_w_lat: f32,   // half-line-width in latitude degrees
    half_w_lon: f32,   // half-line-width in longitude degrees (already / cos(lat))
) -> vec3<f32> {
    var rgb = base;

    // ── Grid step based on zoom (radius as proxy) ──────────────────────────
    //   radius < 120 px → 30°,  < 300 px → 15°,  ≥ 300 px → 10°
    let gs = select(
        select(30.0f, 15.0f, u.radius >= 120.0),
        10.0f,
        u.radius >= 300.0
    );

    // ── Special latitude parallels (equator, tropics, polar circles) ───────
    let specials = array<f32, 5>(0.0, 23.44, -23.44, 66.56, -66.56);
    for (var i = 0u; i < 5u; i++) {
        let a = line_alpha(abs(lat_deg - specials[i]), half_w_lat * 1.6) * 0.78;
        rgb = mix(rgb, u.hot_col.rgb, a);
    }

    // ── Prime meridian ─────────────────────────────────────────────────────
    {
        let a = line_alpha(abs(lon_deg), half_w_lon * 1.6) * 0.78;
        rgb = mix(rgb, u.hot_col.rgb, a);
    }

    // ── Major lat/lon lines (30° grid) ─────────────────────────────────────
    {
        let major_col = u.grid_col.rgb * 2.5;
        let lat_a = line_alpha(deg_dist_to_step(lat_deg, 30.0), half_w_lat) * 0.55;
        rgb = mix(rgb, major_col, lat_a);

        let lon_a = line_alpha(deg_dist_to_step(lon_deg, 30.0), half_w_lon) * 0.55;
        rgb = mix(rgb, major_col, lon_a);
    }

    // ── Minor lat/lon lines (grid_step) ────────────────────────────────────
    if gs < 30.0 {
        let minor_col = u.grid_col.rgb;
        let lat_a = line_alpha(deg_dist_to_step(lat_deg, gs), half_w_lat) * 0.30;
        rgb = mix(rgb, minor_col, lat_a);

        let lon_a = line_alpha(deg_dist_to_step(lon_deg, gs), half_w_lon) * 0.30;
        rgb = mix(rgb, minor_col, lon_a);
    }

    return rgb;
}

// ── Fragment shader ─────────────────────────────────────────────────────────
@fragment
fn fs_main(@builtin(position) frag_pos: vec4<f32>) -> @location(0) vec4<f32> {
    // Convert physical-pixel position to logical pixels.
    let ppp  = u.pixels_per_point;
    let lx   = frag_pos.x / ppp;
    let ly   = frag_pos.y / ppp;
    let dx   = lx - u.center.x;
    let dy   = ly - u.center.y;

    // ── Build ray through this pixel ───────────────────────────────────────
    // Camera at (0, 0, camera_distance) in globe space (unit sphere at origin).
    // Projection: screen_x = center_x - globe_x * (radius*focal_length / depth)
    //             screen_y = center_y - globe_y * (radius*focal_length / depth)
    // Inversion:  rx/rz = -dx/(radius*focal_length),  ry/rz = -dy/(radius*focal_length),
    //             with rz = -1 (ray points toward –z).
    let inv_f = 1.0 / (u.radius * u.focal_length);
    let rx = -dx * inv_f;
    let ry = -dy * inv_f;
    let rz = -1.0f;

    // ── Ray–sphere intersection ────────────────────────────────────────────
    // |cam + t*ray|² = 1,  cam = (0,0,cam_dist)
    // a·t² + b·t + c = 0
    let a    = rx*rx + ry*ry + 1.0;          // rz² = 1
    let b    = -2.0 * u.camera_distance;     // 2·rz·cam_dist = −2·cam_dist
    let c    = u.camera_distance * u.camera_distance - 1.0;
    let disc = b*b - 4.0*a*c;

    if disc < 0.0 {
        // ── Outside sphere — soft atmospheric halo ─────────────────────────
        let edge_dist = sqrt(dx*dx + dy*dy) / u.radius - 1.0;
        let halo      = exp(-edge_dist * 9.0) * 0.14;
        // Premultiplied alpha
        return vec4<f32>(u.grid_col.rgb * halo * 1.4, halo);
    }

    // Front-face hit (smaller t).
    let t   = (-b - sqrt(disc)) / (2.0 * a);
    let hit = vec3<f32>(rx * t, ry * t, u.camera_distance - t); // rz*t = –t

    // ── Un-rotate hit to canonical (geographic) sphere coords ─────────────
    // Forward rotations applied in globe_scene::project_geo:
    //   1. yaw   about y-axis
    //   2. pitch about x-axis
    // Inverse: un-pitch then un-yaw.
    let cp = cos(u.pitch); let sp = sin(u.pitch);
    let cy = cos(u.yaw);   let sy = sin(u.yaw);

    // Un-pitch (rotate by –pitch about x-axis)
    let y1 =  hit.y * cp + hit.z * sp;
    let z1 = -hit.y * sp + hit.z * cp;

    // Un-yaw (rotate by –yaw about y-axis)
    let x2 =  hit.x * cy - z1 * sy;
    let z2 =  hit.x * sy + z1 * cy;
    let y2 =  y1;

    // ── Geographic coordinates ─────────────────────────────────────────────
    let lat_rad = asin(clamp(y2, -1.0, 1.0));
    let lon_rad = atan2(z2, x2);
    let lat_deg = lat_rad * (180.0 / PI);
    let lon_deg = lon_rad * (180.0 / PI);

    // ── Terrain elevation & base colour ───────────────────────────────────
    let elev   = terrain_elev(lat_deg, lon_deg);
    let e_norm = elev / 1.6;
    var rgb    = surface_color(e_norm);

    // ── Lambertian shading (sun in canonical space) ────────────────────────
    // Fixed sun direction: high, slightly south-west, in front of the globe.
    let sun   = normalize(vec3<f32>(0.32, 0.55, 0.77));
    let shade = clamp(dot(vec3<f32>(x2, y2, z2), sun), 0.0, 1.0);
    rgb = rgb * (0.30 + 0.70 * shade);

    // ── Rim atmosphere (view-space fresnel) ───────────────────────────────
    // Camera is at (0,0,cam_dist) in view space; hit is on unit sphere.
    let to_cam  = normalize(vec3<f32>(-hit.x, -hit.y, u.camera_distance - hit.z));
    let rim     = 1.0 - clamp(dot(hit, to_cam), 0.0, 1.0);
    let atm     = pow(rim, 3.5) * 0.42;
    rgb = mix(rgb, u.grid_col.rgb * 2.2, atm);

    // ── Graticule ──────────────────────────────────────────────────────────
    if u.show_graticule != 0u {
        // Approximate degrees-per-pixel at this lat/lon.
        // At the globe centre, 1 logical pixel ≈ (180/π) / radius degrees.
        let deg_per_px  = (180.0 / PI) / u.radius;
        let cos_lat     = max(cos(lat_rad), 0.05); // avoid /0 near poles
        let half_w_lat  = deg_per_px * 1.1;
        let half_w_lon  = deg_per_px * 1.1 / cos_lat;

        rgb = apply_graticule(rgb, lat_deg, lon_deg, half_w_lat, half_w_lon);
    }

    // ── Sphere-edge stroke (replaces circle_stroke in draw_backdrop) ───────
    // Thin brightening at the very limb.
    let edge_a = smoothstep(0.09, 0.0, rim - 0.92) * 0.55;
    rgb = mix(rgb, u.mount_col.rgb * 0.85, edge_a);

    // Fully opaque inside the sphere (premultiplied: colour*1, alpha=1).
    return vec4<f32>(rgb, 1.0);
}
"#;
