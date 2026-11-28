// Unified RTC GPU point-cloud sprite renderer for the fused highest-density
// LiDAR point cloud sidebar layer (default point size = 1 px).
//
// Concatenated after `unified_camera.wgsl` (provides `RtcCamera` & Reversed-Z helpers).
// Must match `PointCloudUniforms` in `point_cloud_pass.rs` byte-for-byte (288 bytes).

struct PointCloudUniforms {
    cam: RtcCamera,              // 112 bytes
    // x = point_size_px (default 1.0), y = min_elev_m, z = max_elev_m, w = opacity
    params: vec4<f32>,           // 16 bytes
    // x = class_mask, y = color_mode (0=Class, 1=Height, 2=RGB, 3=Intensity, 4=Heatmap),
    // z = 0, w = 0
    flags: vec4<u32>,            // 16 bytes
    heat_colors: array<vec4<f32>, 7>, // 112 bytes
    // Padding to 288 bytes (16-byte aligned).
    extra: vec4<f32>,            // 16 bytes
    extra2: vec4<f32>,           // 16 bytes
}

@group(0) @binding(0) var<uniform> u: PointCloudUniforms;

struct VsIn {
    @builtin(vertex_index) vidx: u32,
    @location(0) local_elev: vec4<f32>, // xyz = tile-local sea-level ECEF m, w = elev_m
    @location(1) color: vec4<f32>,      // RGB / fallback color (unorm8x4)
    @location(2) packed: u32,           // bits 0..7 = class, 8..23 = intensity, 31 = has_rgb
}

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) corner: vec2<f32>,
}

fn class_color(classification: u32) -> vec3<f32> {
    if classification == 2u { return vec3<f32>(0.71, 0.60, 0.45); }
    if classification >= 3u && classification <= 5u { return vec3<f32>(0.30, 0.71, 0.52); }
    if classification == 6u { return vec3<f32>(0.86, 0.66, 0.41); }
    if classification == 9u { return vec3<f32>(0.29, 0.64, 0.86); }
    return vec3<f32>(0.44, 0.78, 0.84);
}

fn elevation_heatmap(z: f32) -> vec3<f32> {
    var t = 0.5;
    if u.params.z > u.params.y {
        t = clamp((z - u.params.y) / (u.params.z - u.params.y), 0.0, 1.0);
    }
    let stop = t * 6.0;
    let i = min(u32(stop), 5u);
    return mix(u.heat_colors[i].rgb, u.heat_colors[i + 1u].rgb, stop - f32(i));
}

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    let classification = in.packed & 255u;
    if (u.flags.x & (1u << min(classification, 31u))) == 0u {
        out.clip = vec4<f32>(0.0, 0.0, -1.0, 1.0);
        out.color = vec4<f32>(0.0);
        out.corner = vec2<f32>(0.0);
        return out;
    }

    let displaced = rtc_displace_elevation(u.cam, in.local_elev.xyz, in.local_elev.w);
    let clip = rtc_project_clip(u.cam, displaced);
    if u.cam.proj.w < 0.5 && clip.w <= u.cam.proj.z * 0.5 {
        out.clip = vec4<f32>(0.0, 0.0, -1.0, 1.0);
        out.color = vec4<f32>(0.0);
        out.corner = vec2<f32>(0.0);
        return out;
    }

    // 3-vertex circumscribing triangle (for size == 1 px we keep a tight 1x1 pixel footprint).
    let corners = array<vec2<f32>, 3>(
        vec2<f32>(-1.7320508, -1.0),
        vec2<f32>( 1.7320508, -1.0),
        vec2<f32>( 0.0,        2.0),
    );
    let corner = corners[in.vidx];
    let radius_px = max(0.5, u.params.x * 0.5);
    let viewport_px = vec2<f32>(u.cam.cam_right.w, u.cam.cam_up.w);

    // Slight positive depth nudge in Reversed-Z so points resting on terrain are never Z-fought out.
    let z_nudge = select(0.00008 * clip.w, 0.00008, u.cam.proj.w > 0.5);
    out.clip = vec4<f32>(
        clip.xy + corner * (radius_px * 2.0 / viewport_px) * clip.w,
        clip.z + z_nudge,
        clip.w,
    );
    out.corner = corner;

    var rgb: vec3<f32>;
    switch u.flags.y {
        case 1u: {
            let span = max(u.params.z - u.params.y, 1.0);
            let t = clamp((in.local_elev.w - u.params.y) / span, 0.0, 1.0);
            rgb = mix(vec3<f32>(0.15, 0.48, 0.68), vec3<f32>(0.98, 0.45, 0.16), t);
        }
        case 2u: {
            if (in.packed & 0x80000000u) != 0u {
                rgb = in.color.rgb;
            } else {
                rgb = class_color(classification);
            }
        }
        case 3u: {
            let intensity = sqrt(f32((in.packed >> 8u) & 65535u) / 65535.0);
            rgb = vec3<f32>(intensity);
        }
        case 4u: {
            rgb = elevation_heatmap(in.local_elev.w);
        }
        default: {
            rgb = class_color(classification);
        }
    }
    out.color = vec4<f32>(rgb * u.params.w, u.params.w);
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    if u.params.x > 1.25 && dot(in.corner, in.corner) > 1.0 {
        discard;
    }
    return in.color;
}
