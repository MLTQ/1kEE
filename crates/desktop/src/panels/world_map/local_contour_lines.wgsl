// Instanced anti-aliased segment renderer for the local terrain scene.
//
// The globe's `contour_lines.wgsl` projects unit-sphere endpoints through a
// perspective camera. This one projects raw geographic endpoints — (lon, lat,
// elevation_m) — through the local scene's oblique tangent-plane transform,
// which is the exact arithmetic of `LocalProjector::project` in
// local_terrain_scene/projection.rs. That function and the `project` below must
// stay in step; `local_projection_matches_the_shader` in projection.rs pins the
// scalar half of it.
//
// The Rust `LocalContourUniforms` struct in local_contour_pass.rs must stay
// byte-for-byte in sync with `Uniforms` below (112 bytes).

struct Uniforms {
    focus_lon: f32,
    focus_lat: f32,
    x_factor: f32,
    y_factor: f32,
    z_factor: f32,
    yaw_cos: f32,
    yaw_sin: f32,
    pitch_cos: f32,
    pitch_sin: f32,
    focus_center_x: f32,        // logical points
    focus_center_y: f32,        // logical points
    horizontal_scale: f32,
    ground_pitch_scale: f32,
    ground_depth_scale: f32,
    elevation_pitch_scale: f32,
    elevation_depth_scale: f32,
    // egui-wgpu runs paint callbacks with the GPU viewport set to the callback
    // rect, so NDC must be computed against that rect — not the full window.
    viewport_min_x: f32,        // physical pixels
    viewport_min_y: f32,
    viewport_size_x: f32,
    viewport_size_y: f32,
    // Major and minor contours carry different widths. The CPU width formula
    // ends in a max() floor, so the two are computed host-side and selected
    // per instance rather than derived from one width by a multiplier.
    stroke_half_px_minor: f32,  // half stroke width, physical pixels
    stroke_half_px_major: f32,
    feather_px_minor: f32,      // AA feather, physical pixels
    feather_px_major: f32,
    pixels_per_point: f32,
    alpha: f32,                 // pass fade multiplier
    _pad0: f32,
    _pad1: f32,
}

@group(0) @binding(0) var<uniform> u: Uniforms;

struct VsIn {
    @builtin(vertex_index) vidx: u32,
    @location(0) a: vec3<f32>,      // endpoint A: (lon, lat, elevation_m)
    @location(1) b: vec3<f32>,      // endpoint B: (lon, lat, elevation_m)
    @location(2) color: vec4<f32>,  // premultiplied linear RGBA
    @location(3) major: u32,        // 1 for major contours, 0 for minor
}

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) dist_px: f32,      // signed perpendicular distance from centreline
    // Constant across the quad; carried through so the fragment shader knows
    // which width this instance was expanded with.
    @location(2) @interpolate(flat) stroke_half_px: f32,
    @location(3) @interpolate(flat) feather_px: f32,
}

// Project (lon, lat, elevation_m) to physical-pixel screen space.
// Returns (x_px, y_px, valid); valid < 0 means the point projected to a
// degenerate or wildly out-of-range position.
fn project(geo: vec3<f32>) -> vec3<f32> {
    let x = (geo.x - u.focus_lon) * u.x_factor;
    let y = (geo.y - u.focus_lat) * u.y_factor;
    let z = geo.z * u.z_factor;

    let x_yaw = x * u.yaw_cos - y * u.yaw_sin;
    let y_yaw = x * u.yaw_sin + y * u.yaw_cos;

    let ground_y_pitch = y_yaw * u.pitch_cos;
    let ground_z_pitch = y_yaw * u.pitch_sin;
    let elevation_y_offset = z * u.pitch_sin;
    let elevation_z_offset = z * u.pitch_cos;

    // Positive y is north, mapped upward on screen by negating the ground terms.
    let logical = vec2<f32>(
        u.focus_center_x + x_yaw * u.horizontal_scale,
        u.focus_center_y
            - ground_y_pitch * u.ground_pitch_scale
            + ground_z_pitch * u.ground_depth_scale
            - elevation_y_offset * u.elevation_pitch_scale
            - elevation_z_offset * u.elevation_depth_scale,
    );

    // The CPU path rejects points far outside the layout as blown projections.
    // The rasterizer would discard them anyway, but keeping the guard avoids
    // feeding non-finite values into the quad expansion below.
    let px = logical * u.pixels_per_point;
    let span = max(u.viewport_size_x, u.viewport_size_y) * 8.0;
    if (!(abs(px.x) < span && abs(px.y) < span)) {
        return vec3<f32>(0.0, 0.0, -1.0);
    }
    return vec3<f32>(px, 1.0);
}

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    let pa = project(in.a);
    let pb = project(in.b);
    if (pa.z < 0.0 || pb.z < 0.0) {
        // Behind the clip volume so the quad rasterizes nothing, matching the
        // CPU path dropping the segment.
        out.clip = vec4<f32>(0.0, 0.0, 2.0, 1.0);
        out.color = vec4<f32>(0.0);
        out.dist_px = 0.0;
        out.stroke_half_px = 0.0;
        out.feather_px = 1.0;
        return out;
    }

    // Quad corner from vertex index: two triangles (0,1,2) (3,4,5).
    // t = position along the segment, side = perpendicular sign.
    var t: f32;
    var side: f32;
    switch in.vidx {
        case 0u: { t = 0.0; side = -1.0; }
        case 1u: { t = 1.0; side = -1.0; }
        case 2u: { t = 1.0; side = 1.0; }
        case 3u: { t = 0.0; side = -1.0; }
        case 4u: { t = 1.0; side = 1.0; }
        default: { t = 0.0; side = 1.0; }
    }

    let is_major = in.major != 0u;
    let stroke_half_px = select(u.stroke_half_px_minor, u.stroke_half_px_major, is_major);
    let feather_px = select(u.feather_px_minor, u.feather_px_major, is_major);

    var dir = pb.xy - pa.xy;
    let len = length(dir);
    if (len < 1e-4) {
        dir = vec2<f32>(1.0, 0.0);
    } else {
        dir = dir / len;
    }
    let normal = vec2<f32>(-dir.y, dir.x);
    let half_extent = stroke_half_px + feather_px;
    // Extend endpoints lengthwise by the feather to hide joint cracks on smooth
    // contours — but never by more than half the segment, or a segment shorter
    // than its own feather inflates into a blob roughly `2 * feather` across.
    // Densely sampled contours are mostly such segments, which is what made
    // them read as far heavier than an isolated line at the same width.
    let extend = min(feather_px, len * 0.5);
    let px = mix(pa.xy, pb.xy, t)
        + dir * (t * 2.0 - 1.0) * extend
        + normal * side * half_extent;

    let rel = (px - vec2<f32>(u.viewport_min_x, u.viewport_min_y))
        / vec2<f32>(u.viewport_size_x, u.viewport_size_y);
    let ndc = vec2<f32>(rel.x * 2.0 - 1.0, 1.0 - rel.y * 2.0);
    out.clip = vec4<f32>(ndc, 0.0, 1.0);
    out.color = in.color;
    out.dist_px = side * half_extent;
    out.stroke_half_px = stroke_half_px;
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
    // A stroke narrower than a pixel cannot be drawn narrower than a pixel; it
    // has to be drawn *fainter*. Taken from the per-instance width so major and
    // minor contours each get their own factor. At a pixel and above it is 1.0.
    let sub_pixel = clamp(2.0 * in.stroke_half_px, 0.0, 1.0);
    // Colour is premultiplied — scale the whole vector by coverage x pass fade.
    return in.color * (edge * u.alpha * sub_pixel);
}
