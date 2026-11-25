struct Params { n: u32, width: u32, height: u32, origin: u32 }
@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var<storage, read> source: array<u32>;
// Local source-pixel coordinates, calculated in f64 on the host. Negative =
// another source owns this output pixel. No global lon/lat arithmetic in f32.
@group(0) @binding(2) var<storage, read> axes: array<f32>;
@group(0) @binding(3) var<storage, read_write> raster: array<f32>;
// Native signed elevations are packed two per word, including the -32768 void.
fn height_at(i: u32) -> f32 {
    return f32(extractBits(bitcast<i32>(source[i / 2u]), 16u * (i % 2u), 16u));
}
@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let ox = (p.origin & 0xffffu) + id.x;
    let oy = (p.origin >> 16u) + id.y;
    if ox >= p.n || oy >= p.n { return; }
    let x = axes[ox]; let y = axes[p.n + oy];
    if x < 0.0 || y < 0.0 { return; }
    let x0 = u32(x); let y0 = u32(y);
    let x1 = min(x0 + 1u, p.width - 1u);
    let y1 = min(y0 + 1u, p.height - 1u);
    let a = height_at(y0 * p.width + x0); let b = height_at(y0 * p.width + x1);
    let c = height_at(y1 * p.width + x0); let d = height_at(y1 * p.width + x1);
    let tx = x - f32(x0); let ty = y - f32(y0);
    // Ignore invalid neighbors only when their contribution is exactly zero.
    // This preserves valid SRTM posts adjacent to voids on the native grid.
    var value = -32768.0;
    if a > -32767.0 && (tx == 0.0 || b > -32767.0)
        && (ty == 0.0 || c > -32767.0)
        && (tx == 0.0 || ty == 0.0 || d > -32767.0) {
        let top = a + (b - a) * tx;
        let bottom = c + (d - c) * tx;
        value = top + (bottom - top) * ty;
    }
    if value > -32767.0 { raster[oy * p.n + ox] = value; }
}
