// GPU Viewshed / Line-of-Sight compute shader with Earth curvature & atmospheric refraction.
// Dispatched at @workgroup_size(8, 8) over a DEM height raster (R32Float).

struct ViewshedUniforms {
    observer_uv: vec2<f32>,
    observer_elev_m: f32,
    max_range_m: f32,
    bounds: vec4<f32>,      // (west, south, east, north) degrees
    inv_2_r_eff: f32,       // 1.0 / (2.0 * R_earth / 0.87) for curvature + refraction
    steps: u32,
    _pad0: u32,
    _pad1: u32,
}

@group(0) @binding(0) var<uniform> u: ViewshedUniforms;
@group(0) @binding(1) var heights: texture_2d<f32>;
@group(0) @binding(2) var out_vis: texture_storage_2d<rgba8unorm, write>;

fn sample_height(uv: vec2<f32>, size: vec2<i32>) -> f32 {
    let p = clamp(uv * vec2<f32>(size) - 0.5, vec2<f32>(0.0), vec2<f32>(size - 1));
    let c = vec2<i32>(floor(p));
    let n = min(c + 1, size - 1);
    let t = fract(p);
    let a = textureLoad(heights, c, 0).r;
    let b = textureLoad(heights, vec2<i32>(n.x, c.y), 0).r;
    let d0 = textureLoad(heights, vec2<i32>(c.x, n.y), 0).r;
    let d1 = textureLoad(heights, n, 0).r;
    return mix(mix(a, b, t.x), mix(d0, d1, t.x), t.y);
}

@compute @workgroup_size(8, 8)
fn cs_viewshed(@builtin(global_invocation_id) gid: vec3<u32>) {
    let size = vec2<i32>(textureDimensions(heights));
    if (i32(gid.x) >= size.x || i32(gid.y) >= size.y) {
        return;
    }
    let uv = (vec2<f32>(gid.xy) + 0.5) / vec2<f32>(size);
    let target_h = sample_height(uv, size);
    let mid_lat = radians((u.bounds.y + u.bounds.w) * 0.5);
    let m_per_deg_lat = 111195.0;
    let m_per_deg_lon = m_per_deg_lat * max(abs(cos(mid_lat)), 0.05);
    let d_uv = uv - u.observer_uv;
    let dx_m = d_uv.x * (u.bounds.z - u.bounds.x) * m_per_deg_lon;
    let dy_m = -d_uv.y * (u.bounds.w - u.bounds.y) * m_per_deg_lat;
    let dist_m = sqrt(dx_m * dx_m + dy_m * dy_m);

    if (dist_m > u.max_range_m) {
        textureStore(out_vis, vec2<i32>(gid.xy), vec4<f32>(0.0, 0.0, 0.0, 0.0));
        return;
    }
    if (dist_m < 25.0) {
        textureStore(out_vis, vec2<i32>(gid.xy), vec4<f32>(0.0, 0.96, 0.85, 0.85));
        return;
    }

    let target_eff = target_h - dist_m * dist_m * u.inv_2_r_eff;
    let target_angle = (target_eff - u.observer_elev_m) / dist_m;
    let num_steps = max(u.steps, 8u);
    var visible = true;
    for (var i = 1u; i < num_steps; i += 1u) {
        let t = f32(i) / f32(num_steps);
        let s_uv = mix(u.observer_uv, uv, t);
        let s_dist = dist_m * t;
        let s_h = sample_height(s_uv, size) - s_dist * s_dist * u.inv_2_r_eff;
        let s_angle = (s_h - u.observer_elev_m) / max(s_dist, 1.0);
        if (s_angle > target_angle) {
            visible = false;
            break;
        }
    }

    let fade = 1.0 - smoothstep(u.max_range_m * 0.82, u.max_range_m, dist_m);
    if (visible) {
        textureStore(out_vis, vec2<i32>(gid.xy), vec4<f32>(0.18, 0.92, 0.45, 0.42 * fade));
    } else {
        textureStore(out_vis, vec2<i32>(gid.xy), vec4<f32>(0.85, 0.16, 0.28, 0.14 * fade));
    }
}
