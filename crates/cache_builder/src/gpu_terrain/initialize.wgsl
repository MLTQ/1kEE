struct Params { n: u32, unused1: u32, unused2: u32, unused3: u32 }
@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var<storage, read_write> raster: array<f32>;
@compute @workgroup_size(16,16)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x < p.n && id.y < p.n { raster[id.y*p.n+id.x] = -32768.0; }
}
