struct Uniforms {
    center: vec2<f32>, origin: vec2<f32>, size: vec2<f32>, rotation: vec2<f32>,
    pitch: vec2<f32>, radius_focal: f32, distance: f32, opacity: f32,
    linear_target: f32, pad1: f32, pad2: f32,
}
@group(0) @binding(0) var<uniform> u: Uniforms;
struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) sphere: vec3<f32>,
    @location(1) @interpolate(flat) color: vec4<f32>,
}
fn rotate_sphere(p: vec3<f32>) -> vec3<f32> {
    let x = p.x * u.rotation.y + p.z * u.rotation.x;
    let z = -p.x * u.rotation.x + p.z * u.rotation.y;
    return vec3<f32>(x, p.y * u.pitch.y - z * u.pitch.x, p.y * u.pitch.x + z * u.pitch.y);
}
@vertex fn vs_main(@builtin(vertex_index) i: u32, @location(0) a: vec3<f32>,
    @location(1) b: vec3<f32>, @location(2) c: vec3<f32>, @location(3) color: vec4<f32>) -> VertexOut {
    var out: VertexOut;
    let qa = rotate_sphere(a);
    let qb = rotate_sphere(b);
    let qc = rotate_sphere(c);
    // Back-hemisphere early cull: if all three vertices sit behind the perspective limb
    // (with sagitta margin for 2° chord subdivision), discard the triangle before rasterization.
    let limb = 1.0 / u.distance - 0.002;
    if max(qa.z, max(qb.z, qc.z)) < limb {
        out.position = vec4<f32>(0.0, 0.0, 2.0, 1.0);
        out.sphere = vec3<f32>(0.0);
        out.color = vec4<f32>(0.0);
        return out;
    }
    var q = qa;
    if i == 1u { q = qb; }
    if i == 2u { q = qc; }
    let depth = u.distance - q.z;
    let pixel = u.center - q.xy * u.radius_focal / depth;
    let ndc = vec2<f32>(2.0 * (pixel.x - u.origin.x) / u.size.x - 1.0, 1.0 - 2.0 * (pixel.y - u.origin.y) / u.size.y);
    // Color32::from_rgba_unmultiplied premultiplies in linear light,
    // then stores gamma-encoded RGB. Compute once per vertex (flat) instead of per fragment.
    let linear = color * u.opacity;
    let low = 12.92 * linear.rgb;
    let high = 1.055 * pow(linear.rgb, vec3<f32>(1.0 / 2.4)) - vec3<f32>(0.055);
    let gamma = select(high, low, linear.rgb <= vec3<f32>(0.0031308));
    let quantized = floor(gamma * 255.0 + vec3<f32>(0.5)) / 255.0;
    var final_rgb = quantized;
    if u.linear_target > 0.5 {
        let lo = quantized / 12.92;
        let hi = pow((quantized + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
        final_rgb = select(hi, lo, quantized <= vec3<f32>(0.04045));
    }
    out.position = vec4<f32>(ndc * depth, 0.0, depth);
    out.sphere = q;
    out.color = vec4<f32>(final_rgb, linear.a);
    return out;
}
@fragment fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    // Perspective limb: visible surface normals satisfy normalize(sphere).z >= 1/distance.
    // Equivalent without inverseSqrt: z >= 0 && z^2 * distance^2 >= dot(sphere, sphere).
    if in.sphere.z < 0.0 || (in.sphere.z * in.sphere.z) * (u.distance * u.distance) < dot(in.sphere, in.sphere) {
        discard;
    }
    return in.color;
}
