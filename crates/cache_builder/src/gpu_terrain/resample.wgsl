struct Params { n: u32, width: u32, height: u32, unused: u32 }
@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var<storage, read> source: array<f32>;
// Local source-pixel coordinates, calculated in f64 on the host. Negative =
// another source owns this output pixel. No global lon/lat arithmetic in f32.
@group(0) @binding(2) var<storage, read> axes: array<f32>;
@group(0) @binding(3) var<storage, read_write> raster: array<f32>;
@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= p.n || id.y >= p.n { return; }
    let x = axes[id.x]; let y = axes[p.n + id.y];
    if x < 0.0 || y < 0.0 { return; }
    let x0 = u32(x); let y0 = u32(y);
    let x1 = min(x0 + 1u, p.width - 1u);
    let y1 = min(y0 + 1u, p.height - 1u);
    let a = source[y0 * p.width + x0]; let b = source[y0 * p.width + x1];
    let c = source[y1 * p.width + x0]; let d = source[y1 * p.width + x1];
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
    if value > -32767.0 { raster[id.y * p.n + id.x] = value; }
}
