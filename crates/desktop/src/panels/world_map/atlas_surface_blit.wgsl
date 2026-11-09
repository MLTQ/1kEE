@group(0) @binding(0) var picture:texture_2d<f32>;
@group(0) @binding(1) var picture_sampler:sampler;
struct Output {@builtin(position) position:vec4<f32>,@location(0) uv:vec2<f32>};
@vertex
fn vs_blit(@builtin(vertex_index) vertex:u32)->Output {
    var positions=array<vec2<f32>,3>(vec2<f32>(-1.0,-1.0),vec2<f32>(3.0,-1.0),vec2<f32>(-1.0,3.0));
    let p=positions[vertex];var out:Output;out.position=vec4<f32>(p,0.0,1.0);
    out.uv=vec2<f32>(p.x*0.5+0.5,0.5-p.y*0.5);return out;
}
@fragment
fn fs_blit(v:Output)->@location(0) vec4<f32> {
    return map_output_gamma(textureSample(picture,picture_sampler,v.uv));
}
