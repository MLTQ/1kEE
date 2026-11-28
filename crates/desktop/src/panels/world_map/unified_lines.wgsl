// Unified RTC instanced anti-aliased line renderer for all geographic vector layers
// (coastlines, bathymetry, global topo, SRTM/3DEP/SLDEM/CTX contours, roads,
// admin borders, pipelines) across the entire planet-to-metre zoom range.
//
// Concatenated after `unified_camera.wgsl` (which provides `RtcCamera` & helpers).
// Must match `UnifiedLineUniforms` in `unified_line_pass.rs` byte-for-byte (208 bytes).

struct LineUniforms {
    cam: RtcCamera,
    // x = stroke_half_px_minor, y = stroke_half_px_major,
    // z = feather_px_minor,     w = feather_px_major
    stroke: vec4<f32>,
    // x = alpha, y = depth_bias_ndc (positive pushes closer in Reversed-Z),
    // z = tanaka_on (>0.5), w = abyss_glow (>0.5)
    style: vec4<f32>,
    // x = palette_override (>0.5), y = horizon_cull (>0.5), z = extra_radial_m, w = 0.0
    flags: vec4<f32>,
    major_color: vec4<f32>,
    minor_color: vec4<f32>,
    // Unit vector from planet center toward camera eye (for horizon culling); w = 0.0.
    eye_unit: vec4<f32>,
}

@group(0) @binding(0) var<uniform> u: LineUniforms;

struct VsIn {
    @builtin(vertex_index) vidx: u32,
    @location(0) a_local_elev: vec4<f32>, // xyz = tile-local sea-level m, w = elev_m
    @location(1) b_local_elev: vec4<f32>, // xyz = tile-local sea-level m, w = elev_m
    @location(2) color: vec4<f32>,        // premultiplied display RGBA
    @location(3) flags: u32,              // bit 0 = major contour
}

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) dist_px: f32,
    @location(2) @interpolate(flat) stroke_half_px: f32,
    @location(3) @interpolate(flat) feather_px: f32,
}

fn eval_endpoint(local_elev: vec4<f32>) -> vec4<f32> {
    let normal = normalize(u.cam.tile_normal.xyz + local_elev.xyz * u.cam.tile_normal.w);
    // Optional planet-horizon cull for far-hemisphere vectors.
    if u.flags.y > 0.5 {
        let horizon_dot = select(
            dot(normal, u.eye_unit.xyz),
            dot(normal, -u.cam.cam_forward.xyz),
            u.cam.proj.w > 0.5,
        );
        if horizon_dot < u.cam.depth_ctrl.z - 0.015 {
            return vec4<f32>(0.0, 0.0, -1.0, -1.0);
        }
    }
    let displaced = local_elev.xyz + normal * (local_elev.w * u.cam.depth_ctrl.y + u.flags.z);
    let clip = rtc_project_clip(u.cam, displaced);
    if u.cam.proj.w < 0.5 && clip.w <= u.cam.proj.z * 0.5 {
        return vec4<f32>(0.0, 0.0, -1.0, -1.0);
    }
    let ndc = clip.xyz / clip.w;
    let px = vec2<f32>(
        (ndc.x * 0.5 + 0.5) * u.cam.cam_right.w,
        (0.5 - ndc.y * 0.5) * u.cam.cam_up.w,
    );
    return vec4<f32>(px, clamp(ndc.z + u.style.y, 0.0, 0.999999), 1.0);
}

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    let pa = eval_endpoint(in.a_local_elev);
    let pb = eval_endpoint(in.b_local_elev);
    if pa.w < 0.0 || pb.w < 0.0 {
        out.clip = vec4<f32>(0.0, 0.0, -1.0, 1.0);
        out.color = vec4<f32>(0.0);
        out.dist_px = 0.0;
        out.stroke_half_px = 0.0;
        out.feather_px = 1.0;
        return out;
    }

    let t = f32(in.vidx & 1u);
    let side = select(-1.0, 1.0, in.vidx >= 2u);
    let is_major = (in.flags & 1u) != 0u;
    let stroke_half_px = select(u.stroke.x, u.stroke.y, is_major);
    let feather_px = select(u.stroke.z, u.stroke.w, is_major);

    var dir = pb.xy - pa.xy;
    let len = length(dir);
    if len < 1e-4 {
        dir = vec2<f32>(1.0, 0.0);
    } else {
        dir = dir / len;
    }
    let normal = vec2<f32>(-dir.y, dir.x);

    var base_color = in.color;
    if u.flags.x > 0.5 {
        base_color = select(u.minor_color, u.major_color, is_major);
    }
    var width_mult = 1.0;
    if u.style.z > 0.5 {
        let lit = clamp(dot(normal, vec2<f32>(-0.70710678, -0.70710678)), -1.0, 1.0);
        let a = max(base_color.a, 0.001);
        let hi = vec3<f32>(0.98, 0.97, 0.92) * a;
        let sh = base_color.rgb * 0.22;
        let mixed = select(mix(base_color.rgb, sh, -lit * 0.78), mix(base_color.rgb, hi, lit * 0.65), lit >= 0.0);
        base_color = vec4<f32>(mixed, base_color.a);
        width_mult = 0.85 + 0.35 * abs(lit);
    }

    let abyss_glow = u.style.w > 0.5;
    let extra_feather = select(feather_px, max(feather_px * 2.2, 2.0), abyss_glow);
    let half_extent = stroke_half_px * width_mult + extra_feather;
    let extend = min(feather_px, len * 0.5);
    let px = mix(pa.xy, pb.xy, t)
        + dir * (t * 2.0 - 1.0) * extend
        + normal * side * half_extent;

    let ndc = vec2<f32>(
        (px.x / u.cam.cam_right.w) * 2.0 - 1.0,
        1.0 - (px.y / u.cam.cam_up.w) * 2.0,
    );
    let depth = mix(pa.z, pb.z, t);
    out.clip = vec4<f32>(ndc, depth, 1.0);
    out.color = base_color;
    out.dist_px = side * half_extent;
    out.stroke_half_px = stroke_half_px * width_mult;
    out.feather_px = feather_px;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let d = abs(in.dist_px);
    let edge = clamp(
        (in.stroke_half_px + in.feather_px - d) / max(in.feather_px, 0.001),
        0.0,
        1.0,
    );
    let sub_pixel = clamp(2.0 * in.stroke_half_px, 0.0, 1.0);
    var cov = edge * sub_pixel;
    if u.style.w > 0.5 {
        let glow = exp(-d / max(in.stroke_half_px * 1.4, 0.85)) * 0.36;
        cov = max(cov, glow);
    }
    if cov * u.style.x < 0.002 {
        discard;
    }
    return in.color * (cov * u.style.x);
}
