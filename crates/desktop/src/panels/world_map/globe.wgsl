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
        if u.active_body != 0u || u.atlas != 0u {
            // Moon and Mars have extremely thin/no atmosphere: pure black beyond the limb.
            return vec4<f32>(0.0, 0.0, 0.0, 0.0);
        }
        // ── Outside sphere — very faint limb halo only ────────────────────
        let edge_dist = sqrt(dx*dx + dy*dy) / u.radius - 1.0;
        let halo      = exp(-edge_dist * 12.0) * 0.045;
        return map_output_linear(vec4<f32>(u.ocean_col.rgb * halo * 2.0 * BACKDROP_EXPOSURE, halo));
    }

    // Front-face hit (smaller t).
    let t   = (-b - sqrt(disc)) / (2.0 * a);
    let hit = vec3<f32>(rx * t, ry * t, u.camera_distance - t); // rz*t = –t

    // Atlas imagery supplies measured relief above this plain paper/ocean base.
    // Missing data must never turn the synthetic backdrop into an atlas.
    if u.atlas != 0u && u.active_body == 0u {
        return map_output_linear(vec4<f32>(u.ocean_col.rgb, 1.0));
    }

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

    let to_cam = normalize(vec3<f32>(-hit.x, -hit.y, u.camera_distance - hit.z));
    let rgb = shade_globe(vec3<f32>(x2, y2, z2), dot(hit, to_cam));
    return map_output_linear(vec4<f32>(rgb, 1.0));
}
