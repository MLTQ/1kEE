// Precision-stable RTE ray-sphere planet backdrop supporting Perspective and
// Orthographic projections continuously from orbital distance down to metres.
//
// Concatenated after `globe_surface.wgsl` (which provides `shade_globe`).
// Must match `UnifiedGlobeUniforms` in `unified_globe_pass.rs` byte-for-byte (224 bytes).

struct Uniforms {
    // Camera orthonormal basis in planetary ECEF space.
    //   cam_right.w   = viewport_min.x (logical)
    //   cam_up.w      = viewport_min.y (logical)
    //   cam_forward.w = pixels_per_point
    cam_right: vec4<f32>,
    cam_up: vec4<f32>,
    cam_forward: vec4<f32>,
    // (eye_ecef - target_sea_ecef) in metres — stays tiny near the surface,
    // eliminating catastrophic |eye|^2 - R^2 cancellation down to 1 m altitude!
    //   rel_eye.w = planetary radius R in metres
    rel_eye: vec4<f32>,
    // Unit outward normal at target_sea_ecef; w = 1.0 / R.
    target_normal: vec4<f32>,
    // Perspective:  (tan_half_fov * aspect, tan_half_fov, viewport_w, viewport_h)
    // Orthographic: (half_w_m,              half_h_m,     viewport_w, viewport_h)
    view_span: vec4<f32>,
    // x = is_ortho (0.0 or 1.0), y = halo_scale, z = range_m, w = 0.0
    mode_ctrl: vec4<f32>,
    // Theme colours (linear RGBA) — field names match `globe_surface.wgsl`.
    ocean_col: vec4<f32>,
    land_col: vec4<f32>,
    mount_col: vec4<f32>,
    grid_col: vec4<f32>,
    hot_col: vec4<f32>,
    // Flags — field names match `globe_surface.wgsl`.
    show_graticule: u32,
    active_body: u32,
    atlas: u32,
    _pad: u32,
    // Compatibility field referenced only when `show_graticule != 0u` inside `globe_surface.wgsl`.
    radius: f32,
    _pad1: f32,
    _pad2: f32,
    _pad3: f32,
}

@group(0) @binding(0) var<uniform> u: Uniforms;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) ndc: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
    var verts = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 3.0, -1.0),
        vec2<f32>(-1.0,  3.0),
    );
    let p = verts[vi];
    var out: VsOut;
    out.pos = vec4<f32>(p, 0.0, 1.0);
    out.ndc = p;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let R = u.rel_eye.w;
    let inv_R = u.target_normal.w;
    let n = u.target_normal.xyz;
    let is_ortho = u.mode_ctrl.x > 0.5;

    var o: vec3<f32>;
    var d: vec3<f32>;
    if is_ortho {
        // Start parallel ray well outside the sphere so the front hemisphere hit has t > 0.
        o = u.rel_eye.xyz - u.cam_forward.xyz * (R * 2.5)
            + u.cam_right.xyz * (in.ndc.x * u.view_span.x)
            + u.cam_up.xyz * (in.ndc.y * u.view_span.y);
        d = u.cam_forward.xyz;
    } else {
        o = u.rel_eye.xyz;
        d = normalize(
            u.cam_forward.xyz
            + u.cam_right.xyz * (in.ndc.x * u.view_span.x)
            + u.cam_up.xyz * (in.ndc.y * u.view_span.y)
        );
    }

    // Exact shifted ray-sphere intersection: |R*n + o + t*d|^2 = R^2
    // R^2 cancels identically: t^2 + 2*half_b*t + c = 0.
    let half_b = R * dot(n, d) + dot(o, d);
    let c = 2.0 * R * dot(n, o) + dot(o, o);

    // Compute discriminant carefully to avoid f32 overflow at planetary radii
    // (R ~ 6.37e6 -> R^2 ~ 4e13, comfortably inside f32 max 3.4e38, and scale by inv_R).
    let hb_s = half_b * inv_R;
    let c_s = c * inv_R * inv_R;
    let disc_s = hb_s * hb_s - c_s;

    if disc_s < 0.0 {
        if u.active_body != 0u || u.atlas != 0u {
            discard;
        }
        // Subtle limb atmosphere halo outside the sphere silhouette.
        let miss_r = sqrt(max(-disc_s, 0.0));
        let halo = exp(-miss_r * 14.0) * 0.045;
        return map_output_linear(vec4<f32>(u.ocean_col.rgb * halo * 2.0 * BACKDROP_EXPOSURE, halo));
    }

    let sqrt_disc = sqrt(disc_s) * R;
    // Citardauq's formula avoids catastrophic cancellation when c << half_b^2 (close altitude).
    var t: f32;
    if half_b < 0.0 {
        let denom = -half_b + sqrt_disc;
        t = select(-half_b - sqrt_disc, c / denom, denom > 1e-4 && c > 0.0);
    } else {
        discard;
    }

    if t < 0.0 {
        discard;
    }

    if u.atlas != 0u && u.active_body == 0u {
        return map_output_linear(vec4<f32>(u.ocean_col.rgb, 1.0));
    }

    let hit_unit = normalize(n + (o + d * t) * inv_R);
    let ndotv = clamp(dot(hit_unit, -d), 0.0, 1.0);
    let rgb = shade_globe(hit_unit, ndotv);
    return map_output_linear(vec4<f32>(rgb, 1.0));
}
