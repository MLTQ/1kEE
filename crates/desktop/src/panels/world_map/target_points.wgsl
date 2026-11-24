struct Uniforms {
    eye: vec4<f32>, right: vec4<f32>, up: vec4<f32>, forward: vec4<f32>,
    projection: vec4<f32>, // tan(fov/2), aspect, near, far
    viewport: vec4<f32>, // physical width, height, point radius px, radial metres
    flags: vec4<u32>, // class mask, color mode
    height: vec4<f32>, // Height mode base/span, heatmap low/high
    light: vec4<f32>,
    heat_colors: array<vec4<f32>, 7>,
}
@group(0) @binding(0) var<uniform> u: Uniforms;
struct Input {
    @location(0) position: vec3<f32>, @location(1) color: vec4<f32>, @location(2) packed: u32,
}
struct Output {
    @builtin(position) clip: vec4<f32>, @location(0) color: vec4<f32>,
    @location(1) corner: vec2<f32>, @location(2) xy: vec2<f32>,
    @location(3) world: vec3<f32>,
}
fn projection(position: vec3<f32>) -> vec4<f32> {
    let delta = position - u.eye.xyz;
    let depth = dot(delta, u.forward.xyz);
    // Reversed infinite Z retains depth precision from close inspection out
    // to the whole kilometre, instead of compressing distant points near 1.
    return vec4<f32>(dot(delta, u.right.xyz) / (u.projection.x * u.projection.y),
        dot(delta, u.up.xyz) / u.projection.x, u.projection.z, depth);
}
fn class_color(classification: u32) -> vec4<f32> {
    // Linear RGB, written to an sRGB attachment.
    if classification == 2u { return vec4<f32>(0.462361,0.318547,0.170645,1.); }
    if classification >= 3u && classification <= 5u { return vec4<f32>(0.073239,0.462361,0.233022,1.); }
    if classification == 6u { return vec4<f32>(0.710566,0.393123,0.139985,1.); }
    if classification == 9u { return vec4<f32>(0.068385,0.367246,0.710566,1.); }
    return vec4<f32>(0.162647,0.570482,0.67386,1.);
}
fn linear_rgb(rgb: vec3<f32>) -> vec3<f32> {
    return select(pow((rgb + 0.055) / 1.055, vec3<f32>(2.4)), rgb / 12.92, rgb <= vec3<f32>(0.04045));
}
fn elevation_heatmap(z: f32) -> vec3<f32> {
    var t = 0.5;
    if u.height.w > u.height.z {
        t = clamp((z - u.height.z) / (u.height.w - u.height.z), 0., 1.);
    }
    let stop = t * 6.;
    let i = min(u32(stop), 5u);
    return linear_rgb(mix(u.heat_colors[i].rgb, u.heat_colors[i + 1u].rgb, stop - f32(i)));
}
@vertex fn vs_point(input: Input, @builtin(vertex_index) index: u32) -> Output {
    // 3-vertex circumscribing equilateral triangle inscribes the exact unit circle
    // dot(corner, corner) <= 1.0, cutting vertex invocations in half vs a 6-vert quad.
    let corners = array<vec2<f32>, 3>(
        vec2<f32>(-1.7320508, -1.0),
        vec2<f32>(1.7320508, -1.0),
        vec2<f32>(0.0, 2.0),
    );
    let classification = input.packed & 255u;
    var out: Output;
    let r2 = u.viewport.w * u.viewport.w;
    if (u.flags.x & (1u << min(classification, 31u))) == 0u
        || dot(input.position.xy, input.position.xy) > r2
    {
        out.clip = vec4<f32>(0., 0., 2., 1.);
        out.color = vec4<f32>(0.);
        out.corner = vec2<f32>(0.);
        out.xy = vec2<f32>(0.);
        out.world = vec3<f32>(0.);
        return out;
    }
    let clip = projection(input.position);
    let corner = corners[index];
    out.corner = corner;
    out.xy = input.position.xy;
    out.world = input.position;
    out.clip = vec4<f32>(
        clip.xy + corner * u.viewport.z * 2. / u.viewport.xy * clip.w,
        clip.zw,
    );
    switch u.flags.y {
        case 1u: {
            let t = clamp((input.position.z - u.height.x) / max(u.height.y, 1.), 0., 1.);
            out.color = vec4<f32>(
                mix(vec3<f32>(0.021981, 0.223414, 0.354692), vec3<f32>(1., 0.263273, 0.027212), t),
                1.,
            );
        }
        case 2u: {
            if (input.packed & 0x80000000u) != 0u {
                out.color = vec4<f32>(linear_rgb(input.color.rgb), 1.);
            } else {
                out.color = class_color(classification);
            }
        }
        case 3u: {
            let intensity = sqrt(f32((input.packed >> 8u) & 65535u) / 65535.);
            out.color = vec4<f32>(vec3<f32>(intensity), 1.);
        }
        case 4u: {
            out.color = vec4<f32>(elevation_heatmap(input.position.z), 1.);
        }
        default: {
            out.color = class_color(classification);
        }
    }
    return out;
}
@vertex fn vs_surface(input: Input) -> Output {
    var out: Output; out.clip = projection(input.position); out.color = input.color;
    out.corner = vec2<f32>(0.); out.xy = input.position.xy; out.world = input.position; return out;
}
@fragment fn fs_point(input: Output) -> @location(0) vec4<f32> {
    if dot(input.corner, input.corner) > 1. { discard; }
    return input.color;
}
@fragment fn fs_surface(input: Output) -> @location(0) vec4<f32> {
    let normal = normalize(cross(dpdx(input.world), dpdy(input.world)));
    if dot(input.xy, input.xy) > u.viewport.w * u.viewport.w { discard; }
    let up = select(-normal, normal, normal.z >= 0.);
    let relief = normalize(vec3<f32>(up.xy * 3., up.z));
    let diffuse = max(dot(relief, u.light.xyz), 0.);
    let lit = clamp(0.64 + (diffuse - u.light.z) * 1.6, 0.12, 1.15);
    let pigment = mix(vec3<f32>(0.72, 0.81, 0.96), vec3<f32>(1.04, 1.01, 0.95), min(lit, 1.));
    var color = linear_rgb(input.color.rgb);
    if u.flags.y == 4u { color = elevation_heatmap(input.world.z); }
    return vec4<f32>(color * pigment * lit, 1.);
}
@fragment fn fs_lines(input: Output) -> @location(0) vec4<f32> {
    if dot(input.xy, input.xy) > u.viewport.w * u.viewport.w { discard; }
    return vec4<f32>(linear_rgb(input.color.rgb), 1.);
}
