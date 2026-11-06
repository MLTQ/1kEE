// egui uses gamma/display RGB on Unorm targets and compensates only when the
// framebuffer has automatic sRGB encoding. Alpha never has a transfer curve.
override SRGB_FRAMEBUFFER: bool = false;

fn map_linear_rgb(rgb: vec3<f32>) -> vec3<f32> {
    let low = rgb / 12.92;
    let high = pow((rgb + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    return select(high, low, rgb <= vec3<f32>(0.04045));
}
fn map_gamma_rgb(rgb: vec3<f32>) -> vec3<f32> {
    let low = rgb * 12.92;
    let high = 1.055 * pow(max(rgb, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.4)) - vec3<f32>(0.055);
    return select(high, low, rgb <= vec3<f32>(0.0031308));
}
fn map_output_gamma(color: vec4<f32>) -> vec4<f32> {
    if (SRGB_FRAMEBUFFER) { return vec4<f32>(map_linear_rgb(color.rgb), color.a); }
    return color;
}
fn map_output_linear(color: vec4<f32>) -> vec4<f32> {
    if (SRGB_FRAMEBUFFER) { return color; }
    return vec4<f32>(map_gamma_rgb(color.rgb), color.a);
}
// Retained terrain images have the framebuffer's format. Undo an sRGB texture
// sample before interpreting their stored class coverage or diagnostic colors.
fn map_sample_gamma(color: vec4<f32>) -> vec4<f32> {
    if (SRGB_FRAMEBUFFER) { return vec4<f32>(map_gamma_rgb(color.rgb), color.a); }
    return color;
}
