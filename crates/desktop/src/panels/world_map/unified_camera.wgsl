// Shared RTC (Relative-To-Center) 3D curved-planet camera contract.
//
// Byte-for-byte mirror of `RtcCameraUniforms` in `projection.rs` (112 bytes).
// Supports both Perspective (infinite Reversed-Z) and Orthographic (linear
// Reversed-Z) projections on the exact same camera basis and Depth32Float
// attachment (`CompareFunction::GreaterEqual`, clear depth = 0.0).

struct RtcCamera {
    // Orthonormal camera basis in planetary ECEF space.
    //   cam_right.w   = viewport width (physical px)
    //   cam_up.w      = viewport height (physical px)
    //   cam_forward.w = pixels_per_point
    cam_right: vec4<f32>,
    cam_up: vec4<f32>,
    cam_forward: vec4<f32>,
    // (tile_origin_ecef - eye_ecef) computed in f64 on CPU, cast to f32 metres.
    //   rel_origin.w  = planetary radius in metres
    rel_origin: vec4<f32>,
    // Perspective:  (inv_tan_half_fov_x, inv_tan_half_fov_y, near_m, 0.0)
    // Orthographic: (inv_half_w_m,       inv_half_h_m,       z_far_m, 1.0)
    proj: vec4<f32>,
    //   x = ortho_inv_z_span (1.0 / (z_far_m - z_near_m))
    //   y = vertical_exaggeration
    //   z = horizon_cos_min
    //   w = camera_altitude_m
    depth_ctrl: vec4<f32>,
    // Unit outward radial normal at tile_origin_ecef; w = 1.0 / radius_m.
    tile_normal: vec4<f32>,
}

fn rtc_displace_elevation(cam: RtcCamera, local_sea_m: vec3<f32>, elev_m: f32) -> vec3<f32> {
    let normal = normalize(cam.tile_normal.xyz + local_sea_m * cam.tile_normal.w);
    return local_sea_m + normal * (elev_m * cam.depth_ctrl.y);
}

fn rtc_to_camera(cam: RtcCamera, local_m: vec3<f32>) -> vec3<f32> {
    let rel = cam.rel_origin.xyz + local_m;
    return vec3<f32>(
        dot(rel, cam.cam_right.xyz),
        dot(rel, cam.cam_up.xyz),
        dot(rel, cam.cam_forward.xyz),
    );
}

fn rtc_project_clip(cam: RtcCamera, local_m: vec3<f32>) -> vec4<f32> {
    let c = rtc_to_camera(cam, local_m);
    if cam.proj.w > 0.5 {
        let ndc_z = (cam.proj.z - c.z) * cam.depth_ctrl.x;
        return vec4<f32>(c.x * cam.proj.x, c.y * cam.proj.y, ndc_z, 1.0);
    }
    return vec4<f32>(c.x * cam.proj.x, c.y * cam.proj.y, cam.proj.z, c.z);
}
