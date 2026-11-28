// Unified RTC displaced 3D terrain shader for Atlas relief (Overview, 5° GEBCO,
// SRTM, and fine 3DEP/cached DEMs) across the entire planet-to-metre range.
//
// Concatenated with `unified_camera.wgsl` (before) and `atlas_lighting.wgsl` (after).
// Uses exact half-angle spherical offsets relative to each tile's ENU basis so
// both 360° global rasters and 100 m fine DEMs project with sub-mm precision in
// both Perspective and Orthographic Reversed-Z modes.

struct Uniforms {
    cam: RtcCamera,
    // [west_deg, south_deg, east_deg, north_deg] of the raster texture.
    region: vec4<f32>,
    // [core_west, core_south, core_east, core_north] drawn by this mesh;
    // when extra.w < 0.5, draws full `region`.
    core: vec4<f32>,
    // Tile-origin ENU basis computed in f64 on CPU (exact at tile center).
    //   tile_east.w  = tile_center_lon_rad
    //   tile_north.w = tile_center_lat_rad
    //   tile_up.w    = cos(tile_center_lat_rad)
    tile_east: vec4<f32>,
    tile_north: vec4<f32>,
    tile_up: vec4<f32>,
    // x = sin(tile_center_lat_rad), y = grid_cols, z = grid_rows, w = coast_enabled
    grid: vec4<f32>,
    // x = contour_interval_m, y = contour_width_px, z = contours_enabled, w = bathy_enabled
    style: vec4<f32>,
    // x = color_richness, y = relief_strength, z = forest_enabled, w = analysis_tanaka_flags
    appearance: vec4<f32>,
    // Directional/sun lighting parameters consumed by `atlas_lighting.wgsl`.
    light: vec4<f32>,
    major: vec4<f32>,
    minor: vec4<f32>,
    // Exclude bbox (discard fragments covered by a finer child tile).
    exclude: vec4<f32>,
    // x = exclude_mode (0 = none, 1 = bbox exclude, 2 = 5° tile_coverage mask),
    // y = use_core_bounds (>0.5), z = height_scale_m_mult, w = 0.0
    extra: vec4<f32>,
}

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var tile_coverage: texture_2d<u32>;
@group(1) @binding(0) var heights: texture_2d<f32>;
@group(1) @binding(1) var pigments: texture_2d<f32>;
@group(1) @binding(2) var linear_sampler: sampler;
@group(1) @binding(3) var forest_coverage: texture_2d<f32>;

struct Vertex {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) elevation: f32,
    @location(2) geo: vec2<f32>,
}

fn safe_height(h: f32, fallback: f32) -> f32 {
    if h >= -11000.0 && h <= 9000.0 { return h; }
    return fallback;
}

fn height_at_or(coordinate: vec2<f32>, fallback: f32) -> f32 {
    var uv = coordinate;
    if u.region.z - u.region.x > 359.9 { uv.x = fract(uv.x); }
    let size = vec2<i32>(textureDimensions(heights));
    let p = clamp(uv * vec2<f32>(size) - 0.5, vec2<f32>(0.0), vec2<f32>(size - 1));
    let cell = vec2<i32>(floor(p));
    let next = min(cell + 1, size - 1);
    let t = fract(p);
    let a = safe_height(textureLoad(heights, cell, 0).r, fallback);
    let b = safe_height(textureLoad(heights, vec2<i32>(next.x, cell.y), 0).r, fallback);
    let c = safe_height(textureLoad(heights, vec2<i32>(cell.x, next.y), 0).r, fallback);
    let d = safe_height(textureLoad(heights, next, 0).r, fallback);
    return mix(mix(a, b, t.x), mix(c, d, t.x), t.y);
}

fn height_at(uv: vec2<f32>) -> f32 { return height_at_or(uv, 0.0); }

@vertex
fn vs_surface(@builtin(vertex_index) vertex: u32, @builtin(instance_index) row: u32) -> Vertex {
    let cells = u.grid.yz;
    let coord = vec2<f32>(f32(vertex / 2u), f32(row + (vertex % 2u))) / cells;

    var draw_bounds = u.region;
    if u.extra.y > 0.5 {
        draw_bounds = u.core;
    }
    let geo = vec2<f32>(
        mix(draw_bounds.x, draw_bounds.z, coord.x),
        mix(draw_bounds.w, draw_bounds.y, coord.y),
    );
    let uv = vec2<f32>(
        (geo.x - u.region.x) / max(u.region.z - u.region.x, 1e-7),
        (u.region.w - geo.y) / max(u.region.w - u.region.y, 1e-7),
    );

    // Exact cancellation-free spherical displacement from tile center (lat0, lon0).
    let span_lon = radians(draw_bounds.z - draw_bounds.x);
    let span_lat = radians(draw_bounds.w - draw_bounds.y);
    let dlon = (coord.x - 0.5) * span_lon;
    let dlat = (0.5 - coord.y) * span_lat;

    let s_lat = sin(dlat);
    let hs_lat = sin(0.5 * dlat);
    let omc_lat = 2.0 * hs_lat * hs_lat;

    let s_lon = sin(dlon);
    let hs_lon = sin(0.5 * dlon);
    let omc_lon = 2.0 * hs_lon * hs_lon;

    let c0 = u.tile_up.w;
    let s0 = u.grid.x;
    let c_lat = c0 * (1.0 - omc_lat) - s0 * s_lat;

    let de = c_lat * s_lon;
    let dn = s_lat + s0 * c_lat * omc_lon;
    let du = -(omc_lat + c0 * c_lat * omc_lon);

    let R = u.cam.rel_origin.w;
    let local_sea_m = R * (u.tile_east.xyz * de + u.tile_north.xyz * dn + u.tile_up.xyz * du);

    let h = height_at(uv);
    let displaced = rtc_displace_elevation(u.cam, local_sea_m, h * u.extra.z);

    var out: Vertex;
    out.clip = rtc_project_clip(u.cam, displaced);
    out.uv = uv;
    out.elevation = h;
    out.geo = geo;
    return out;
}

fn isoline(h: f32, interval: f32) -> f32 {
    let dh = fwidth(h);
    let level = h / interval;
    let width = max(fwidth(level), 0.00001);
    let distance = abs(fract(level + 0.5) - 0.5);
    let slope_mask = smoothstep(0.0002, 0.002, dh);
    let non_zero_level = step(0.5, abs(round(level)));
    return (1.0 - smoothstep(width * u.style.y * 0.2, width * (u.style.y * 0.5 + 0.6), distance))
        * min(1.0, 0.6 / width)
        * slope_mask
        * non_zero_level;
}

@fragment
fn fs_surface(v: Vertex) -> @location(0) vec4<f32> {
    if u.extra.x > 1.5 {
        let lon = ((v.geo.x + 540.0) % 360.0);
        let cell = clamp(vec2<i32>(floor(vec2<f32>(lon, v.geo.y + 90.0) / 5.0)), vec2<i32>(0), vec2<i32>(71, 35));
        if textureLoad(tile_coverage, cell, 0).r > 0u { discard; }
    } else if u.extra.x > 0.5 {
        let center = (u.exclude.x + u.exclude.z) * 0.5;
        let lon = center + ((v.geo.x - center + 540.0) % 360.0) - 180.0;
        if lon >= u.exclude.x && lon <= u.exclude.z && v.geo.y >= u.exclude.y && v.geo.y <= u.exclude.w {
            discard;
        }
    }

    let pixel = max(vec2<f32>(1.0), fwidth(v.uv) * vec2<f32>(textureDimensions(heights)));
    let interval = select(max(200.0, u.style.x), u.style.x, v.elevation >= 0.0);
    let minor = isoline(v.elevation, interval);
    let major = isoline(v.elevation, max(50.0, interval * 10.0));
    let dh = fwidth(v.elevation);
    let coast = (1.0 - smoothstep(max(dh * 0.2, 0.001), max(dh, 0.002), abs(v.elevation)))
        * step(0.001, dh)
        * u.grid.w;

    var color = textureSampleLevel(pigments, linear_sampler, v.uv, 0.0);
    if color.a < 0.5 { discard; }
    if u.appearance.z > 0.5 {
        let coverage = textureSampleLevel(forest_coverage, linear_sampler, v.uv, 0.0).r;
        color = vec4<f32>(mix(color.rgb, vec3<f32>(0.22, 0.36, 0.20), coverage * 0.74), 1.0);
    }

    let flags = u32(round(max(u.appearance.w, 0.0)));
    let tanaka = (flags & 1u) != 0u;
    let analysis = (flags >> 1u) & 3u;
    var slope_vec = vec2<f32>(0.0);
    if tanaka || analysis != 0u {
        let size = vec2<f32>(textureDimensions(heights));
        let spacing = max(
            vec2<f32>(0.05),
            vec2<f32>(
                (u.region.z - u.region.x) * 111195.0 * max(abs(cos(radians(v.geo.y))), 0.05),
                (u.region.w - u.region.y) * 111195.0,
            ) / size,
        );
        let h_c = height_at(v.uv);
        let fine_radius = max(vec2<f32>(1.0), pixel * 0.7);
        slope_vec = atlas_slope(v.uv, h_c, spacing, fine_radius);
        if analysis == 1u {
            let deg = degrees(atan(length(slope_vec)));
            let t = clamp(deg / 50.0, 0.0, 1.0);
            let low = vec3<f32>(0.12, 0.18, 0.28);
            let mid = vec3<f32>(0.86, 0.42, 0.16);
            let high = vec3<f32>(0.99, 0.92, 0.48);
            let ramp = select(mix(low, mid, t * 2.0), mix(mid, high, (t - 0.5) * 2.0), t >= 0.5);
            color = vec4<f32>(mix(color.rgb, ramp, 0.78), 1.0);
        } else if analysis == 2u {
            let mag = length(slope_vec);
            let ang = atan2(-slope_vec.x, -slope_vec.y);
            let wheel = 0.5 + 0.5 * cos(ang + vec3<f32>(0.0, 2.094, 4.188));
            let flat_fade = smoothstep(0.02, 0.18, mag);
            color = vec4<f32>(mix(color.rgb, wheel, 0.72 * flat_fade), 1.0);
        } else if analysis == 3u {
            let view_span_m = min(spacing.x * size.x, spacing.y * size.y);
            let radius_m = clamp(view_span_m * 0.06, 60.0, 900.0);
            let broad_r = max(fine_radius * 4.0, vec2<f32>(radius_m) / spacing);
            let inv_s = 1.0 / (spacing * size);
            let d_uv = broad_r * spacing * inv_s;
            let mean = (
                height_at_or(v.uv + vec2<f32>(d_uv.x, 0.0), h_c)
                + height_at_or(v.uv - vec2<f32>(d_uv.x, 0.0), h_c)
                + height_at_or(v.uv + vec2<f32>(0.0, d_uv.y), h_c)
                + height_at_or(v.uv - vec2<f32>(0.0, d_uv.y), h_c)
            ) * 0.25;
            let norm_m = clamp(radius_m * 0.12, 12.0, 120.0);
            let tpi = clamp((h_c - mean) / norm_m, -1.0, 1.0);
            let ridge_col = vec3<f32>(0.99, 0.82, 0.34);
            let valley_col = vec3<f32>(0.12, 0.22, 0.52);
            let tpi_col = select(mix(color.rgb, valley_col, abs(tpi) * 0.78), mix(color.rgb, ridge_col, tpi * 0.85), tpi >= 0.0);
            color = vec4<f32>(tpi_col, 1.0);
        }
    }

    color = vec4<f32>(atlas_shade(v.uv, v.geo, color.rgb, pixel), 1.0);
    let enabled = select(u.style.w, u.style.z, v.elevation >= 0.0);
    if tanaka {
        let light = atlas_light(v.geo);
        let grad_len = length(slope_vec);
        let light_len = length(light.xy);
        let lit = select(0.0, clamp(dot(-slope_vec / grad_len, light.xy / light_len), -1.0, 1.0), grad_len > 0.002 && light_len > 0.002);
        let hi_col = vec3<f32>(0.98, 0.96, 0.90);
        let sh_col = vec3<f32>(0.06, 0.08, 0.11);
        let tanaka_minor = mix(sh_col, hi_col, lit * 0.5 + 0.5);
        let tanaka_major = mix(sh_col * 0.8, vec3<f32>(1.0, 0.98, 0.93), lit * 0.5 + 0.5);
        let contrast = 0.45 + 0.55 * abs(lit);
        color = vec4<f32>(mix(color.rgb, tanaka_minor, minor * select(0.24, 0.48, v.elevation >= 0.0) * contrast * enabled), 1.0);
        color = vec4<f32>(mix(color.rgb, tanaka_major, major * select(0.45, 0.78, v.elevation >= 0.0) * contrast * enabled), 1.0);
    } else {
        color = vec4<f32>(mix(color.rgb, u.minor.rgb, minor * select(0.18, 0.32, v.elevation >= 0.0) * enabled), 1.0);
        color = vec4<f32>(mix(color.rgb, u.major.rgb, major * select(0.35, 0.60, v.elevation >= 0.0) * enabled), 1.0);
    }
    color = vec4<f32>(mix(color.rgb, vec3<f32>(0.24, 0.37, 0.40), coast * 0.45), 1.0);
    return color;
}
