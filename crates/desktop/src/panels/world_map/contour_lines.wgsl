// Instanced anti-aliased segment renderer for globe contour layers.
//
// Each instance is one polyline segment: two precomputed unit-sphere endpoints
// plus a baked premultiplied-linear RGBA colour. The vertex shader applies the
// same yaw/pitch rotation + perspective projection as the CPU
// `GlobeXform::apply` (globe_scene/projection.rs), expands the segment into a
// screen-space quad with `feather_px` of anti-alias margin, and culls segments
// that touch the far hemisphere — matching the CPU segment breaker exactly.
//
// The Rust `ContourUniforms` struct in contour_pass.rs must stay byte-for-byte
// in sync with `Uniforms` below (80 bytes).

struct Uniforms {
    center: vec2<f32>,        // globe centre, logical points
    // egui-wgpu runs paint callbacks with the GPU viewport set to the callback
    // rect, so NDC must be computed against that rect — not the full window.
    viewport_min: vec2<f32>,  // callback rect origin, physical pixels
    viewport_size: vec2<f32>, // callback rect size, physical pixels
    yaw_sin: f32,
    yaw_cos: f32,
    pitch_sin: f32,
    pitch_cos: f32,
    radius_focal: f32,        // layout.radius * layout.focal_length
    camera_distance: f32,
    radius_offset: f32,       // per-layer altitude offset (globe-unit fraction)
    alpha: f32,               // layer fade multiplier
    stroke_half_px: f32,      // half stroke width, physical pixels
    feather_px: f32,          // AA feather, physical pixels
    pixels_per_point: f32,
    horizon_z: f32,           // surface pipelines use the perspective horizon
    _pad1: f32,
    _pad2: f32,
}

@group(0) @binding(0) var<uniform> u: Uniforms;

struct VsIn {
    @builtin(vertex_index) vidx: u32,
    @location(0) a: vec3<f32>,      // unit-sphere endpoint A
    @location(1) b: vec3<f32>,      // unit-sphere endpoint B
    @location(2) color: vec4<f32>,  // exact stored Color32 display RGBA
}

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) dist_px: f32,      // signed perpendicular distance from centreline
}

// Project a unit-sphere point to physical-pixel screen space.
// Returns (x_px, y_px, valid); valid < 0 means near-plane hit or back-facing.
fn project(unit: vec3<f32>) -> vec3<f32> {
    let p = unit * (1.0 + u.radius_offset);
    let x1 = p.x * u.yaw_cos + p.z * u.yaw_sin;
    let z1 = -p.x * u.yaw_sin + p.z * u.yaw_cos;
    let y2 = p.y * u.pitch_cos - z1 * u.pitch_sin;
    let z2 = p.y * u.pitch_sin + z1 * u.pitch_cos;
    let depth = u.camera_distance - z2;
    if (depth <= 0.05 || z2 < u.horizon_z) {
        return vec3<f32>(0.0, 0.0, -1.0);
    }
    let perspective = u.radius_focal / depth;
    let logical = vec2<f32>(u.center.x - x1 * perspective, u.center.y - y2 * perspective);
    return vec3<f32>(logical * u.pixels_per_point, 1.0);
}

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    let pa = project(in.a);
    let pb = project(in.b);
    if (pa.z < 0.0 || pb.z < 0.0) {
        // Whole segment culled, same as the CPU path breaker. Behind the
        // clip volume so the quad rasterizes nothing.
        out.clip = vec4<f32>(0.0, 0.0, 2.0, 1.0);
        out.color = vec4<f32>(0.0);
        out.dist_px = 0.0;
        return out;
    }

    // Four-corner strip: two triangles share their endpoints.
    // t = position along the segment, side = perpendicular sign.
    var t: f32;
    var side: f32;
    switch in.vidx {
        case 0u: { t = 0.0; side = -1.0; }
        case 1u: { t = 1.0; side = -1.0; }
        case 2u: { t = 0.0; side = 1.0; }
        default: { t = 1.0; side = 1.0; }
    }

    var dir = pb.xy - pa.xy;
    let len = length(dir);
    if (len < 1e-4) {
        dir = vec2<f32>(1.0, 0.0);
    } else {
        dir = dir / len;
    }
    let normal = vec2<f32>(-dir.y, dir.x);
    let tanaka_on = u._pad1 > 0.5;
    let abyss_glow = u._pad2 > 0.5;
    var shade_color = in.color;
    var width_mult = 1.0;
    if (tanaka_on) {
        // Screen-space NW key light (-0.7071, -0.7071): illuminated ridges brighten, shadowed edges darken.
        let lit = clamp(dot(normal, vec2<f32>(-0.70710678, -0.70710678)), -1.0, 1.0);
        let a = max(in.color.a, 0.001);
        let hi = vec3<f32>(0.98, 0.97, 0.92) * a;
        let sh = in.color.rgb * 0.22;
        let mixed = select(mix(in.color.rgb, sh, -lit * 0.78), mix(in.color.rgb, hi, lit * 0.65), lit >= 0.0);
        shade_color = vec4<f32>(mixed, in.color.a);
        width_mult = 0.85 + 0.35 * abs(lit);
    }
    let extra_feather = select(u.feather_px, max(u.feather_px * 2.2, 2.0), abyss_glow);
    let half_extent = u.stroke_half_px * width_mult + extra_feather;
    // Extend endpoints lengthwise by the feather to hide joint cracks on smooth
    // contours — but never by more than half the segment, or a segment shorter
    // than its own feather inflates into a blob roughly `2 * feather` across.
    // Densely sampled contours are mostly such segments, which is what made
    // them read as far heavier than an isolated line at the same width.
    let extend = min(u.feather_px, len * 0.5);
    let px = mix(pa.xy, pb.xy, t)
        + dir * (t * 2.0 - 1.0) * extend
        + normal * side * half_extent;

    let rel = (px - u.viewport_min) / u.viewport_size;
    let ndc = vec2<f32>(rel.x * 2.0 - 1.0, 1.0 - rel.y * 2.0);
    out.clip = vec4<f32>(ndc, 0.0, 1.0);
    out.color = shade_color;
    out.dist_px = side * half_extent;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let d = abs(in.dist_px);
    let edge = clamp((u.stroke_half_px + u.feather_px - d) / max(u.feather_px, 0.001), 0.0, 1.0);
    // A stroke narrower than a pixel cannot be drawn narrower than a pixel; it
    // has to be drawn *fainter*, which is how sub-pixel line rendering works.
    // Without this a 0.25 px setting still paints whichever pixels the quad
    // happens to cover at full opacity, giving a broken one-pixel line rather
    // than a fine one. At a pixel and above this is exactly 1.0.
    let sub_pixel = clamp(2.0 * u.stroke_half_px, 0.0, 1.0);
    var cov = edge * sub_pixel;
    if (u._pad2 > 0.5) {
        let glow = exp(-d / max(u.stroke_half_px * 1.4, 0.85)) * 0.36;
        cov = max(cov, glow);
    }
    // Colour is premultiplied — scale the whole vector by coverage × layer fade.
    return map_output_gamma(in.color * (cov * u.alpha));
}
