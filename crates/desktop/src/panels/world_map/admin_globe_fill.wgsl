struct Uniforms {
    center: vec2<f32>, origin: vec2<f32>, size: vec2<f32>, rotation: vec2<f32>,
    pitch: vec2<f32>, radius_focal: f32, distance: f32, opacity: f32,
    linear_target: f32, pad1: f32, pad2: f32,
}
@group(0) @binding(0) var<uniform> u: Uniforms;
struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) sphere: vec3<f32>,
    @location(1) color: vec4<f32>,
}
@vertex fn vs_main(@builtin(vertex_index) i: u32, @location(0) a: vec3<f32>,
    @location(1) b: vec3<f32>, @location(2) c: vec3<f32>, @location(3) color: vec4<f32>) -> VertexOut {
    var p = a;
    if i == 1u { p = b; }
    if i == 2u { p = c; }
    let x = p.x*u.rotation.y + p.z*u.rotation.x;
    let z = -p.x*u.rotation.x + p.z*u.rotation.y;
    let q = vec3<f32>(x, p.y*u.pitch.y-z*u.pitch.x, p.y*u.pitch.x+z*u.pitch.y);
    let depth = u.distance-q.z;
    let pixel = u.center-q.xy*u.radius_focal/depth;
    let ndc = vec2<f32>(2.0*(pixel.x-u.origin.x)/u.size.x-1.0, 1.0-2.0*(pixel.y-u.origin.y)/u.size.y);
    var out: VertexOut;
    out.position = vec4<f32>(ndc*depth, 0.0, depth);
    out.sphere = q;
    out.color = color;
    return out;
}
@fragment fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    // Perspective limb: visible surface normals satisfy dot(n, camera) >= 1.
    if normalize(in.sphere).z < 1.0/u.distance { discard; }
    // Color32::from_rgba_unmultiplied premultiplies in linear light,
    // then stores gamma-encoded RGB. Match that before framebuffer conversion.
    let linear = in.color*u.opacity;
    let low = 12.92*linear.rgb;
    let high = 1.055*pow(linear.rgb,vec3<f32>(1.0/2.4))-vec3<f32>(0.055);
    let gamma = select(high,low,linear.rgb<=vec3<f32>(0.0031308));
    let quantized = floor(gamma*255.0+vec3<f32>(0.5))/255.0;
    if u.linear_target > 0.5 {
        let lo = quantized/12.92;
        let hi = pow((quantized+vec3<f32>(0.055))/1.055,vec3<f32>(2.4));
        return vec4<f32>(select(hi,lo,quantized<=vec3<f32>(0.04045)),linear.a);
    }
    return vec4<f32>(quantized,linear.a);
}
