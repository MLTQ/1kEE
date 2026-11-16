struct Uniforms {
    eye: vec4<f32>, right: vec4<f32>, up: vec4<f32>, forward: vec4<f32>,
    projection: vec4<f32>, // tan(fov/2), aspect, near, far
    viewport: vec4<f32>, // physical width, height, point radius px, radial metres
    flags: vec4<u32>, // class mask, color mode
    height: vec4<f32>,
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
@vertex fn vs_point(input: Input, @builtin(vertex_index) index: u32) -> Output {
    let corners = array<vec2<f32>,6>(vec2<f32>(-1.,-1.),vec2<f32>(1.,-1.),vec2<f32>(-1.,1.),
        vec2<f32>(-1.,1.),vec2<f32>(1.,-1.),vec2<f32>(1.,1.));
    let classification = input.packed & 255u;
    var out: Output;
    out.clip = projection(input.position);
    out.corner = corners[index]; out.xy = input.position.xy; out.world = input.position;
    out.clip = vec4<f32>(out.clip.xy + out.corner * u.viewport.z * 2. / u.viewport.xy * out.clip.w,
        out.clip.zw);
    if (u.flags.x & (1u << min(classification,31u))) == 0u {
        out.clip = vec4<f32>(0.,0.,2.,1.);
    }
    out.color = class_color(classification);
    if u.flags.y == 1u {
        let t = clamp((input.position.z - u.height.x) / max(u.height.y,1.),0.,1.);
        out.color = vec4<f32>(mix(vec3<f32>(0.021981,0.223414,0.354692),vec3<f32>(1.,0.263273,0.027212),t),1.);
    }
    if u.flags.y == 2u && (input.packed & 0x80000000u) != 0u { out.color = vec4<f32>(linear_rgb(input.color.rgb),1.); }
    if u.flags.y == 3u {
        let intensity = sqrt(f32((input.packed >> 8u) & 65535u) / 65535.);
        out.color = vec4<f32>(vec3<f32>(intensity),1.);
    }
    return out;
}
@vertex fn vs_surface(input: Input) -> Output {
    var out: Output; out.clip = projection(input.position); out.color = input.color;
    out.corner = vec2<f32>(0.); out.xy = input.position.xy; out.world = input.position; return out;
}
@fragment fn fs_point(input: Output) -> @location(0) vec4<f32> {
    if dot(input.corner,input.corner) > 1. || length(input.xy) > u.viewport.w { discard; }
    return input.color;
}
@fragment fn fs_surface(input: Output) -> @location(0) vec4<f32> {
    let normal = normalize(cross(dpdx(input.world),dpdy(input.world)));
    let illumination = 0.25 + 0.75 * abs(dot(normal,normalize(vec3<f32>(0.4,-0.5,0.75))));
    if length(input.xy) > u.viewport.w { discard; }
    return vec4<f32>(linear_rgb(input.color.rgb) * illumination,1.);
}
@fragment fn fs_lines(input: Output) -> @location(0) vec4<f32> {
    if length(input.xy) > u.viewport.w { discard; }
    return vec4<f32>(linear_rgb(input.color.rgb),1.);
}
