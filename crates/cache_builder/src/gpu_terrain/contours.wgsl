struct Params { n: u32, capacity: u32, interval: f32, unused: u32 }
struct Segment { ax: f32, ay: f32, bx: f32, by: f32, level: f32 }
@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var<storage, read> raster: array<f32>;
@group(0) @binding(2) var<storage, read_write> segments: array<Segment>;
@group(0) @binding(3) var<storage, read_write> count: atomic<u32>;
fn interpolate(a: f32, b: f32, h: f32) -> f32 {
    // Work relative to the level so the tie perturbation survives f32 at
    // high elevations. GDAL considers samples equal to a level above it.
    var da = a-h; var db = b-h;
    if abs(da) < 0.000001 { da += 0.000001; }
    if abs(db) < 0.000001 { db += 0.000001; }
    if abs(db-da) < 0.0000001 { return 0.5; }
    return clamp(-da/(db-da), 0.0, 1.0);
}
fn emit(a: vec2<f32>, b: vec2<f32>, h: f32) {
    if atomicLoad(&count) > p.capacity { return; }
    let i = atomicAdd(&count, 1u);
    if i < p.capacity { segments[i] = Segment(a.x, a.y, b.x, b.y, h); }
}
fn contour(tl: f32, tr: f32, br: f32, bl: f32, x: f32, y: f32, step: f32, h: f32) {
    let code = select(0u,8u,tl>=h) | select(0u,4u,tr>=h)
             | select(0u,2u,br>=h) | select(0u,1u,bl>=h);
    if code == 0u || code == 15u { return; }
    let n = vec2(x + step * interpolate(tl,tr,h), y);
    let e = vec2(x + step, y + step * interpolate(tr,br,h));
    let s = vec2(x + step * interpolate(bl,br,h), y + step);
    let w = vec2(x, y + step * interpolate(tl,bl,h));
    switch code {
        case 1u,14u: { emit(w,s,h); }
        case 2u,13u: { emit(s,e,h); }
        case 3u,12u: { emit(w,e,h); }
        case 4u,11u: { emit(n,e,h); }
        case 6u,9u: { emit(n,s,h); }
        case 7u,8u: { emit(n,w,h); }
        case 5u,10u: {
            // Fixed diagonal agrees with GDAL and avoids level crossings.
            emit(w,s,h); emit(n,e,h);
        }
        default: {}
    }
}
fn cell(tl: f32, tr: f32, br: f32, bl: f32, x: f32, y: f32, step: f32) {
    let low = min(min(tl,tr),min(bl,br)); let high = max(max(tl,tr),max(bl,br));
    var h = ceil(low/p.interval)*p.interval;
    loop {
        if h > high || atomicLoad(&count) > p.capacity { break; }
        contour(tl,tr,br,bl,x,y,step,h);
        h += p.interval;
    }
}
fn midpoint(a: f32,b: f32) -> f32 {
    if a <= -32767.0 { return b; }
    if b <= -32767.0 { return a; }
    return (a+b)*0.5;
}
@compute @workgroup_size(16,16)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x + 1u >= p.n || id.y + 1u >= p.n { return; }
    let i = id.y*p.n+id.x;
    let tl = raster[i]; let tr = raster[i+1u];
    let bl = raster[i+p.n]; let br = raster[i+p.n+1u];
    let x = f32(id.x); let y = f32(id.y);
    if min(min(tl,tr),min(bl,br)) > -32767.0 {
        cell(tl,tr,br,bl,x,y,1.0); return;
    }
    // Fill only valid corner quarters around a void, stopping at half pixels.
    let valid = select(0.0,1.0,tl > -32767.0) + select(0.0,1.0,tr > -32767.0)
        + select(0.0,1.0,bl > -32767.0) + select(0.0,1.0,br > -32767.0);
    if valid == 0.0 { return; }
    let center = (select(0.0,tl,tl > -32767.0) + select(0.0,tr,tr > -32767.0)
        + select(0.0,bl,bl > -32767.0) + select(0.0,br,br > -32767.0))/valid;
    let n = midpoint(tl,tr); let e = midpoint(tr,br);
    let s = midpoint(bl,br); let w = midpoint(tl,bl);
    if tl > -32767.0 { cell(tl,n,center,w,x,y,0.5); }
    if tr > -32767.0 { cell(n,tr,e,center,x+0.5,y,0.5); }
    if bl > -32767.0 { cell(w,center,s,bl,x,y+0.5,0.5); }
    if br > -32767.0 { cell(center,e,br,s,x+0.5,y+0.5,0.5); }
}
