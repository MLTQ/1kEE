// ── Uniform struct ──────────────────────────────────────────────────────────
//
// This struct MUST stay byte-for-byte identical to `GlobeUniforms` in
// `globe_pass.rs`. See the offset table and the `size_of` assertion there.
// Any field added/removed/reordered must be mirrored in both places.
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
    active_body:    u32,
    atlas: u32,
    _pad1: u32,
}

@group(0) @binding(0) var<uniform> u: Uniforms;

// ── Vertex shader: fullscreen triangle ─────────────────────────────────────
struct VsOut {
    @builtin(position) pos: vec4<f32>,
    // Hoist uniform trig & projection scalars out of the per-pixel fragment loop.
    @location(0) @interpolate(flat) rot: vec4<f32>,    // cp, sp, cy, sy
    @location(1) @interpolate(flat) params: vec4<f32>, // inv_f, b, c, inv_ppp
}

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
    // Three vertices covering the entire NDC clip space.
    // Egui's scissor rect restricts rasterisation to the globe rect.
    var pos = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 3.0, -1.0),
        vec2<f32>(-1.0,  3.0),
    );
    var out: VsOut;
    out.pos = vec4<f32>(pos[vi], 0.0, 1.0);
    out.rot = vec4<f32>(cos(u.pitch), sin(u.pitch), cos(u.yaw), sin(u.yaw));
    out.params = vec4<f32>(
        1.0 / (u.radius * u.focal_length),
        -2.0 * u.camera_distance,
        u.camera_distance * u.camera_distance - 1.0,
        1.0 / u.pixels_per_point,
    );
    return out;
}

// ── Fragment shader ─────────────────────────────────────────────────────────
@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // Convert physical-pixel position to logical pixels.
    let lx   = in.pos.x * in.params.w;
    let ly   = in.pos.y * in.params.w;
    let dx   = lx - u.center.x;
    let dy   = ly - u.center.y;

    // ── Build ray through this pixel ───────────────────────────────────────
    let inv_f = in.params.x;
    let rx = -dx * inv_f;
    let ry = -dy * inv_f;

    // ── Ray–sphere intersection ────────────────────────────────────────────
    // |cam + t*ray|² = 1,  cam = (0,0,cam_dist), rz = -1
    let a    = rx*rx + ry*ry + 1.0;
    let b    = in.params.y;
    let c    = in.params.z;
    let disc = b*b - 4.0*a*c;

    if disc < 0.0 {
        if u.active_body != 0u || u.atlas != 0u {
            // Moon, Mars, and Atlas have no outer atmosphere halo: skip ROP write.
            discard;
        }
        // ── Outside sphere — very faint limb halo only ────────────────────
        let edge_dist = sqrt(dx*dx + dy*dy) / u.radius - 1.0;
        let halo      = exp(-edge_dist * 12.0) * 0.045;
        return map_output_linear(vec4<f32>(u.ocean_col.rgb * halo * 2.0 * BACKDROP_EXPOSURE, halo));
    }

    // Front-face hit (smaller t).
    let sqrt_disc = sqrt(disc);
    let t   = (-b - sqrt_disc) / (2.0 * a);
    let hit = vec3<f32>(rx * t, ry * t, u.camera_distance - t); // rz*t = –t

    // Atlas imagery supplies measured relief above this plain paper/ocean base.
    // Missing data must never turn the synthetic backdrop into an atlas.
    if u.atlas != 0u && u.active_body == 0u {
        return map_output_linear(vec4<f32>(u.ocean_col.rgb, 1.0));
    }

    // ── Un-rotate hit to canonical (geographic) sphere coords ─────────────
    let cp = in.rot.x; let sp = in.rot.y;
    let cy = in.rot.z; let sy = in.rot.w;

    // Un-pitch (rotate by –pitch about x-axis)
    let y1 =  hit.y * cp + hit.z * sp;
    let z1 = -hit.y * sp + hit.z * cp;

    // Un-yaw (rotate by –yaw about y-axis)
    let x2 =  hit.x * cy - z1 * sy;
    let z2 =  hit.x * sy + z1 * cy;
    let y2 =  y1;

    // Exact algebraic identity: dot(hit, normalize(cam - hit)) == sqrt(disc) / (2 * sqrt(a)).
    let ndotv = 0.5 * sqrt_disc * inverseSqrt(a);
    let rgb = shade_globe(vec3<f32>(x2, y2, z2), ndotv);
    return map_output_linear(vec4<f32>(rgb, 1.0));
}
