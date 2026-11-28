//! Unified continuous planet-to-metre 3D camera & RTC projection engine.
//!
//! Replaces the split Globe (unit-sphere f32) and Local (affine tangent plane +
//! 66° polar chart) projections with one `f64` ECEF camera that supports both
//! **Perspective** and **Orthographic** projections seamlessly from orbital
//! range (~25,000 km) down to street/LiDAR scale (~20 m).
//!
//! All GPU passes share [`RtcCameraUniforms`] and `unified_camera.wgsl` with a
//! Reversed-Z `Depth32Float` buffer (`CompareFunction::GreaterEqual`, clear 0).

use crate::model::{
    ActiveBody, GeoPoint, GeoPoint64, GlobeViewState, MIN_CAMERA_RANGE_M, ProjectionMode,
    normalize_lon_f64,
};
use glam::{DVec3, Vec3};

/// Vertical field of view for Perspective mode (38° gives crisp cartographic
/// readability without fisheye distortion and matches legacy globe framing).
pub const VERTICAL_FOV_RAD: f64 = 38.0 * std::f64::consts::PI / 180.0;

/// Byte-for-byte mirror of `RtcCamera` in `unified_camera.wgsl` (112 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct RtcCameraUniforms {
    pub cam_right: [f32; 4],
    pub cam_up: [f32; 4],
    pub cam_forward: [f32; 4],
    pub rel_origin: [f32; 4],
    pub proj: [f32; 4],
    pub depth_ctrl: [f32; 4],
    pub tile_normal: [f32; 4],
}

const _: () = assert!(std::mem::size_of::<RtcCameraUniforms>() == 112);

#[derive(Clone, Copy, Debug)]
pub struct UnifiedProjected {
    pub pos: egui::Pos2,
    /// Reversed-Z NDC depth in `[0, 1]` (`1.0` = near, `0.0` = far).
    pub depth_ndc: f32,
    /// Distance in front of the camera plane in metres (`> 0` is in front).
    pub camera_z: f64,
    /// True when the point is on the visible side of the planet horizon.
    pub front_facing: bool,
}

/// Convert geographic `(lat_deg, lon_deg)` to a unit vector in 1kEE's Y-up
/// planetary coordinate frame (identical to `globe_surface.wgsl` and `vr`).
#[inline]
pub fn geo_to_unit(lat_deg: f64, lon_deg: f64) -> DVec3 {
    let (slat, clat) = lat_deg.to_radians().sin_cos();
    let (slon, clon) = lon_deg.to_radians().sin_cos();
    DVec3::new(clat * clon, slat, clat * slon)
}

/// Inverse of [`geo_to_unit`].
#[inline]
pub fn unit_to_geo(v: DVec3) -> GeoPoint64 {
    let u = v.normalize_or_zero();
    let lat = u.y.clamp(-1.0, 1.0).asin().to_degrees();
    let lon = u.z.atan2(u.x).to_degrees();
    GeoPoint64 {
        lat,
        lon: normalize_lon_f64(lon),
    }
}

/// Convert `(lat_deg, lon_deg, elev_m)` to `f64` ECEF metres on a body of radius `radius_m`.
#[inline]
pub fn geo_to_ecef(lat_deg: f64, lon_deg: f64, elev_m: f64, radius_m: f64) -> DVec3 {
    geo_to_unit(lat_deg, lon_deg) * (radius_m + elev_m)
}

/// Orthonormal local East-North-Up basis at `(lat_deg, lon_deg)` in 1kEE's ECEF frame.
/// Well-defined all the way to `±90°` using `lon_deg` as the reference meridian at the pole.
#[inline]
pub fn local_enu_basis(lat_deg: f64, lon_deg: f64) -> (DVec3, DVec3, DVec3) {
    let (slat, clat) = lat_deg.to_radians().sin_cos();
    let (slon, clon) = lon_deg.to_radians().sin_cos();
    let up = DVec3::new(clat * clon, slat, clat * slon);
    let east = DVec3::new(-slon, 0.0, clon);
    let north = DVec3::new(-slat * clon, clat, -slat * slon);
    (east, north, up)
}

/// Single unified camera projection for one frame.
#[derive(Clone, Copy, Debug)]
pub struct MapProjection {
    pub viewport: egui::Rect,
    pub pixels_per_point: f32,
    pub body: ActiveBody,
    pub radius_m: f64,
    pub projection: ProjectionMode,
    pub target_geo: GeoPoint64,
    pub target_up: DVec3,
    pub target_ecef: DVec3,
    pub eye_ecef: DVec3,
    pub cam_right: DVec3,
    pub cam_up: DVec3,
    pub cam_forward: DVec3,
    pub range_m: f64,
    pub altitude_m: f64,
    pub tan_half_fov: f64,
    pub aspect: f64,
    pub half_h_m: f64,
    pub half_w_m: f64,
    pub near_m: f64,
    pub ortho_z_near: f64,
    pub ortho_z_far: f64,
    pub horizon_cos_min: f64,
    pub vertical_exaggeration: f32,
    pub heading: f32,
    pub tilt: f32,
}

impl MapProjection {
    pub fn from_view(
        view: &GlobeViewState,
        body: ActiveBody,
        viewport: egui::Rect,
        pixels_per_point: f32,
    ) -> Self {
        let radius_m = body.radius_m();
        let max_range = GlobeViewState::max_range_for_body(radius_m);
        let range_m = view.range_m.clamp(MIN_CAMERA_RANGE_M, max_range);
        let max_tilt = GlobeViewState::max_tilt_for_range(range_m, radius_m);
        let tilt = view.tilt.clamp(0.0, max_tilt);
        let heading = view.heading;

        let target_geo = GeoPoint64 {
            lat: view.target_lat.clamp(-90.0, 90.0),
            lon: normalize_lon_f64(view.target_lon),
        };
        let (east, north, radial_up) = local_enu_basis(target_geo.lat, target_geo.lon);

        let (sh, ch) = (heading as f64).sin_cos();
        let horiz_forward = north * ch + east * sh;
        let horiz_right = east * ch - north * sh;

        let (st, ct) = (tilt as f64).sin_cos();
        let to_eye = radial_up * ct - horiz_forward * st;
        let vertical_exaggeration = view.local_layer_spread.max(0.0);
        let target_radius_m = radius_m + view.target_elev_m * (vertical_exaggeration as f64);
        let target_ecef = radial_up * target_radius_m;
        let eye_ecef = target_ecef + to_eye * range_m;
        let cam_forward = -to_eye;
        let cam_right = horiz_right;
        let cam_up = radial_up * st + horiz_forward * ct;

        let eye_len = eye_ecef.length();
        let altitude_m = (eye_len - target_radius_m).max(1.0);

        let w = (viewport.width().max(1.0)) as f64;
        let h = (viewport.height().max(1.0)) as f64;
        let aspect = w / h;
        let tan_half_fov = (VERTICAL_FOV_RAD * 0.5).tan();
        let half_h_m = range_m * tan_half_fov;
        let half_w_m = half_h_m * aspect;

        let near_m = (altitude_m * 0.004)
            .clamp(0.25, 50_000.0)
            .min(range_m * 0.08);
        let ortho_z_half = (half_h_m * 8.0).clamp(60_000.0, radius_m * 1.35);
        let ortho_z_near = range_m - ortho_z_half;
        let ortho_z_far = range_m + ortho_z_half;

        let horizon_cos_min = match view.projection {
            ProjectionMode::Perspective => (radius_m / eye_len).clamp(0.0, 0.999_999),
            ProjectionMode::Orthographic => 0.0,
        };

        Self {
            viewport,
            pixels_per_point: pixels_per_point.max(0.5),
            body,
            radius_m,
            projection: view.projection,
            target_geo,
            target_up: radial_up,
            target_ecef,
            eye_ecef,
            cam_right,
            cam_up,
            cam_forward,
            range_m,
            altitude_m,
            tan_half_fov,
            aspect,
            half_h_m,
            half_w_m,
            near_m,
            ortho_z_near,
            ortho_z_far,
            horizon_cos_min,
            vertical_exaggeration: view.local_layer_spread.max(0.0),
            heading,
            tilt,
        }
    }

    /// Ground metres per logical screen point at the viewport center.
    /// Identical in Perspective and Orthographic modes for seamless switching.
    #[inline]
    pub fn meters_per_point(&self) -> f64 {
        (2.0 * self.half_h_m) / (self.viewport.height().max(1.0) as f64)
    }

    /// Ground metres per physical display pixel at the viewport center.
    #[inline]
    pub fn meters_per_pixel(&self) -> f64 {
        self.meters_per_point() / (self.pixels_per_point as f64)
    }

    /// Approximate angular half-span of the viewport in degrees (for LOD tile tiers).
    #[inline]
    pub fn visible_half_extent_deg(&self) -> f32 {
        let half_rad = (self.half_h_m / self.radius_m).min(std::f64::consts::FRAC_PI_2);
        half_rad.to_degrees() as f32
    }

    /// Build GPU RTC uniforms anchored to `tile_origin_ecef` (usually the tile's
    /// geographic center on the mean sphere). All subtraction happens in `f64`
    /// here so the GPU vertex shader receives small, high-precision `f32` offsets.
    pub fn rtc_uniforms(&self, tile_origin_ecef: DVec3) -> RtcCameraUniforms {
        let rel = tile_origin_ecef - self.eye_ecef;
        let normal = tile_origin_ecef.normalize_or(self.target_up);
        let phys_w = self.viewport.width().max(1.0) * self.pixels_per_point;
        let phys_h = self.viewport.height().max(1.0) * self.pixels_per_point;
        let (proj, ortho_inv_span) = match self.projection {
            ProjectionMode::Perspective => (
                [
                    (1.0 / (self.tan_half_fov * self.aspect)) as f32,
                    (1.0 / self.tan_half_fov) as f32,
                    self.near_m as f32,
                    0.0,
                ],
                0.0_f32,
            ),
            ProjectionMode::Orthographic => (
                [
                    (1.0 / self.half_w_m) as f32,
                    (1.0 / self.half_h_m) as f32,
                    self.ortho_z_far as f32,
                    1.0,
                ],
                (1.0 / (self.ortho_z_far - self.ortho_z_near)) as f32,
            ),
        };
        RtcCameraUniforms {
            cam_right: [
                self.cam_right.x as f32,
                self.cam_right.y as f32,
                self.cam_right.z as f32,
                phys_w,
            ],
            cam_up: [
                self.cam_up.x as f32,
                self.cam_up.y as f32,
                self.cam_up.z as f32,
                phys_h,
            ],
            cam_forward: [
                self.cam_forward.x as f32,
                self.cam_forward.y as f32,
                self.cam_forward.z as f32,
                self.pixels_per_point,
            ],
            rel_origin: [rel.x as f32, rel.y as f32, rel.z as f32, self.radius_m as f32],
            proj,
            depth_ctrl: [
                ortho_inv_span,
                self.vertical_exaggeration,
                self.horizon_cos_min as f32,
                self.altitude_m as f32,
            ],
            tile_normal: [
                normal.x as f32,
                normal.y as f32,
                normal.z as f32,
                (1.0 / self.radius_m) as f32,
            ],
        }
    }

    /// Project a 3D `f64` ECEF position to logical egui coordinates.
    #[inline]
    pub fn project_ecef(&self, p_ecef: DVec3) -> Option<UnifiedProjected> {
        let rel = p_ecef - self.eye_ecef;
        let cx = rel.dot(self.cam_right);
        let cy = rel.dot(self.cam_up);
        let cz = rel.dot(self.cam_forward);

        let (ndc_x, ndc_y, depth_ndc, front_facing) = match self.projection {
            ProjectionMode::Perspective => {
                if cz <= self.near_m * 0.5 {
                    return None;
                }
                let nx = cx / (cz * self.tan_half_fov * self.aspect);
                let ny = cy / (cz * self.tan_half_fov);
                let nz = (self.near_m / cz).clamp(0.0, 1.0) as f32;
                // Exact horizon check: dot(p_unit, eye_unit) >= R / |eye|
                // (with small allowance for elevated points above sea level).
                let p_len = p_ecef.length().max(1.0);
                let eye_len = self.eye_ecef.length().max(1.0);
                let p_unit = p_ecef / p_len;
                let eye_unit = self.eye_ecef / eye_len;
                let r_eff = self.radius_m.min(p_len);
                let min_cos = (r_eff / eye_len).min(1.0) - 0.002;
                let visible = p_unit.dot(eye_unit) >= min_cos;
                (nx, ny, nz, visible)
            }
            ProjectionMode::Orthographic => {
                let nx = cx / self.half_w_m;
                let ny = cy / self.half_h_m;
                let nz = ((self.ortho_z_far - cz) / (self.ortho_z_far - self.ortho_z_near))
                    .clamp(0.0, 1.0) as f32;
                let p_unit = p_ecef.normalize_or_zero();
                let visible = p_unit.dot(-self.cam_forward) >= -0.02;
                (nx, ny, nz, visible)
            }
        };

        if !ndc_x.is_finite() || !ndc_y.is_finite() {
            return None;
        }
        let sx = self.viewport.left() as f64 + (ndc_x * 0.5 + 0.5) * (self.viewport.width() as f64);
        let sy = self.viewport.top() as f64 + (0.5 - ndc_y * 0.5) * (self.viewport.height() as f64);
        Some(UnifiedProjected {
            pos: egui::pos2(sx as f32, sy as f32),
            depth_ndc,
            camera_z: cz,
            front_facing,
        })
    }

    /// Project geographic `(lat, lon, elev_m)` with the current vertical exaggeration.
    #[inline]
    pub fn project_geo(&self, lat_deg: f64, lon_deg: f64, elev_m: f64) -> Option<UnifiedProjected> {
        let p = geo_to_ecef(
            lat_deg,
            lon_deg,
            elev_m * (self.vertical_exaggeration as f64),
            self.radius_m,
        );
        self.project_ecef(p)
    }

    /// Project a sea-level point with zero elevation displacement (matches surface overlays).
    #[inline]
    pub fn project_surface(&self, point: GeoPoint) -> Option<UnifiedProjected> {
        self.project_ecef(geo_to_ecef(point.lat as f64, point.lon as f64, 0.0, self.radius_m))
    }

    /// Project a point radially outward above the surface (e.g. event/camera spires).
    #[inline]
    pub fn project_elevated_radial(
        &self,
        lat_deg: f64,
        lon_deg: f64,
        base_elev_m: f64,
        extra_m: f64,
    ) -> Option<UnifiedProjected> {
        let h = base_elev_m * (self.vertical_exaggeration as f64) + extra_m;
        self.project_ecef(geo_to_ecef(lat_deg, lon_deg, h, self.radius_m))
    }

    /// Construct an `f64` world-space ray `(origin_ecef, unit_dir)` through `screen_pos`.
    /// Works identically for both Perspective and Orthographic projections.
    pub fn unproject_ray(&self, screen_pos: egui::Pos2) -> (DVec3, DVec3) {
        let w = self.viewport.width().max(1.0) as f64;
        let h = self.viewport.height().max(1.0) as f64;
        let ndc_x = ((screen_pos.x as f64 - self.viewport.left() as f64) / w) * 2.0 - 1.0;
        let ndc_y = 1.0 - ((screen_pos.y as f64 - self.viewport.top() as f64) / h) * 2.0;

        match self.projection {
            ProjectionMode::Perspective => {
                let dir = (self.cam_forward
                    + self.cam_right * (ndc_x * self.tan_half_fov * self.aspect)
                    + self.cam_up * (ndc_y * self.tan_half_fov))
                    .normalize();
                (self.eye_ecef, dir)
            }
            ProjectionMode::Orthographic => {
                // Start the parallel ray safely outside the planet so the first
                // intersection is always the front hemisphere.
                let origin = self.target_ecef - self.cam_forward * (self.radius_m * 3.0)
                    + self.cam_right * (ndc_x * self.half_w_m)
                    + self.cam_up * (ndc_y * self.half_h_m);
                (origin, self.cam_forward)
            }
        }
    }

    /// Precision-stable ray–sphere intersection against the target elevation sphere.
    /// Returns the front-face ECEF hit point, or `None` if the ray misses the planet.
    pub fn intersect_sphere(&self, screen_pos: egui::Pos2) -> Option<DVec3> {
        let (origin, dir) = self.unproject_ray(screen_pos);
        intersect_ray_sphere_stable(origin, dir, self.target_ecef.length())
    }

    /// Convert a screen position to geographic `(lat, lon)` on the mean planet sphere.
    pub fn screen_to_geo(&self, screen_pos: egui::Pos2) -> Option<GeoPoint64> {
        self.intersect_sphere(screen_pos).map(unit_to_geo)
    }

    /// Visible geographic bounding box `[west, south, east, north]` in degrees,
    /// handling poles and the antimeridian cleanly.
    pub fn visible_bounds(&self) -> Option<[f64; 4]> {
        let center_lon = self.target_geo.lon;
        let mut west = f64::INFINITY;
        let mut south = f64::INFINITY;
        let mut east = f64::NEG_INFINITY;
        let mut north = f64::NEG_INFINITY;

        let mut include = |p: GeoPoint64| {
            let lon = center_lon + ((p.lon - center_lon + 180.0).rem_euclid(360.0) - 180.0);
            west = west.min(lon);
            south = south.min(p.lat);
            east = east.max(lon);
            north = north.max(p.lat);
        };

        for row in 0..=12 {
            let ty = row as f32 / 12.0;
            let y = egui::lerp(self.viewport.top()..=self.viewport.bottom(), ty);
            for col in 0..=12 {
                let tx = col as f32 / 12.0;
                let x = egui::lerp(self.viewport.left()..=self.viewport.right(), tx);
                if let Some(g) = self.screen_to_geo(egui::pos2(x, y)) {
                    include(g);
                }
            }
        }

        // Check if either geographic pole is inside the viewport.
        for pole_lat in [-90.0_f64, 90.0_f64] {
            if let Some(proj) = self.project_ecef(geo_to_ecef(pole_lat, 0.0, 0.0, self.radius_m))
                && proj.front_facing
                && self.viewport.contains(proj.pos)
            {
                west = -180.0;
                east = 180.0;
                if pole_lat > 0.0 {
                    north = 90.0;
                } else {
                    south = -90.0;
                }
            }
        }

        (west.is_finite() && south.is_finite() && east.is_finite() && north.is_finite())
            .then_some([west, south.clamp(-90.0, 90.0), east, north.clamp(-90.0, 90.0)])
    }

    /// Compute the new `(target_geo, heading)` produced by grabbing the ground
    /// under `from_pos` and dragging it to `to_pos`, using exact 3D spherical
    /// rotation (parallel transport across poles with zero singularities).
    pub fn grab_pan_step(
        &self,
        from_pos: egui::Pos2,
        to_pos: egui::Pos2,
    ) -> Option<(GeoPoint64, f32)> {
        let a = self.intersect_sphere(from_pos)?.normalize();
        let b = self.intersect_sphere(to_pos)?.normalize();
        // Rotate the camera by the inverse of rotating point `a` onto `b`
        // (i.e. rotate target & heading frame from `b` toward `a`).
        let rot_axis = b.cross(a);
        let sin_angle = rot_axis.length();
        let cos_angle = b.dot(a).clamp(-1.0, 1.0);
        if sin_angle < 1e-12 {
            return Some((self.target_geo, self.heading));
        }
        let axis = rot_axis / sin_angle;
        let angle = sin_angle.atan2(cos_angle);

        let (east, north, radial_up) = local_enu_basis(self.target_geo.lat, self.target_geo.lon);
        let (sh, ch) = (self.heading as f64).sin_cos();
        let horiz_forward = north * ch + east * sh;

        let new_up = rotate_around_axis(radial_up, axis, angle).normalize();
        let new_fwd = rotate_around_axis(horiz_forward, axis, angle);
        // Re-project forward into the new tangent plane.
        let new_fwd = (new_fwd - new_up * new_fwd.dot(new_up)).normalize_or_zero();

        let new_geo = unit_to_geo(new_up);
        let (new_east, new_north, _) = local_enu_basis(new_geo.lat, new_geo.lon);
        let new_heading = new_fwd.dot(new_east).atan2(new_fwd.dot(new_north)) as f32;
        Some((new_geo, new_heading))
    }

    /// Pan along screen-plane directions `(dx_right_m, dy_up_m)` via exact
    /// spherical great-circle transport (used for keyboard/momentum coasting).
    pub fn pan_by_meters(&self, right_m: f64, up_m: f64) -> (GeoPoint64, f32) {
        let (east, north, radial_up) = local_enu_basis(self.target_geo.lat, self.target_geo.lon);
        let (sh, ch) = (self.heading as f64).sin_cos();
        let horiz_forward = north * ch + east * sh;
        let horiz_right = east * ch - north * sh;

        let disp = horiz_right * right_m + horiz_forward * up_m;
        let dist = disp.length();
        if dist < 1e-9 {
            return (self.target_geo, self.heading);
        }
        let angle = dist / self.radius_m;
        let dir = disp / dist;
        let axis = radial_up.cross(dir).normalize_or_zero();
        if axis.length_squared() < 0.5 {
            return (self.target_geo, self.heading);
        }

        let new_up = rotate_around_axis(radial_up, axis, angle).normalize();
        let new_fwd = rotate_around_axis(horiz_forward, axis, angle);
        let new_fwd = (new_fwd - new_up * new_fwd.dot(new_up)).normalize_or_zero();

        let new_geo = unit_to_geo(new_up);
        let (new_east, new_north, _) = local_enu_basis(new_geo.lat, new_geo.lon);
        let new_heading = new_fwd.dot(new_east).atan2(new_fwd.dot(new_north)) as f32;
        (new_geo, new_heading)
    }

    /// Format a human-readable scale readout (e.g. `14,500 km`, `42.5 km`, `180 m`).
    pub fn scale_label(&self) -> String {
        let span_m = self.half_h_m * 2.0;
        if span_m >= 100_000.0 {
            format!("{:.0} km", span_m / 1000.0)
        } else if span_m >= 10_000.0 {
            format!("{:.1} km", span_m / 1000.0)
        } else if span_m >= 1_000.0 {
            format!("{:.2} km", span_m / 1000.0)
        } else {
            format!("{:.0} m", span_m)
        }
    }
}

/// Precision-stable ray–sphere intersection (Hearn-Baker closest-approach form).
/// Avoids catastrophic `|origin|^2 - R^2` cancellation when the camera is just
/// metres above a 6,371 km sphere.
pub fn intersect_ray_sphere_stable(origin: DVec3, dir: DVec3, radius: f64) -> Option<DVec3> {
    let d = dir.normalize_or_zero();
    if d.length_squared() < 0.5 {
        return None;
    }
    let b = -origin.dot(d); // distance along ray to closest approach to center
    let perp = origin + d * b;
    let perp_sq = perp.length_squared();
    let r_sq = radius * radius;
    let disc = r_sq - perp_sq;
    if disc < 0.0 {
        return None;
    }
    let half_chord = disc.sqrt();
    let t = b - half_chord;
    if t >= 0.0 {
        Some(origin + d * t)
    } else {
        let t2 = b + half_chord;
        (t2 >= 0.0).then(|| origin + d * t2)
    }
}

/// Rodrigues' rotation formula for rotating `v` around unit vector `axis` by `angle` radians.
#[inline]
pub fn rotate_around_axis(v: DVec3, axis: DVec3, angle: f64) -> DVec3 {
    let (s, c) = angle.sin_cos();
    v * c + axis.cross(v) * s + axis * (axis.dot(v) * (1.0 - c))
}

/// Convert an `f64` ECEF position into tile-local `f32` metres relative to `tile_origin_ecef`.
#[inline]
pub fn ecef_to_tile_local(p_ecef: DVec3, tile_origin_ecef: DVec3) -> Vec3 {
    (p_ecef - tile_origin_ecef).as_vec3()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_rect() -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(40.0, 24.0), egui::vec2(1280.0, 720.0))
    }

    #[test]
    fn round_trip_project_and_unproject_from_planet_to_50m_in_both_modes() {
        let rect = test_rect();
        for mode in [ProjectionMode::Perspective, ProjectionMode::Orthographic] {
            for &range_m in &[20_000_000.0, 1_500_000.0, 120_000.0, 5_000.0, 50.0] {
                let mut view = GlobeViewState::from_focus(GeoPoint {
                    lat: 35.6762,
                    lon: 139.6503,
                });
                view.range_m = range_m;
                view.heading = 0.42;
                view.tilt = GlobeViewState::max_tilt_for_range(range_m, ActiveBody::Earth.radius_m())
                    .min(0.65);
                view.projection = mode;

                let proj = MapProjection::from_view(&view, ActiveBody::Earth, rect, 2.0);
                // Test center and an off-center point.
                let offset_deg = (proj.visible_half_extent_deg() as f64 * 0.25).min(8.0);
                let sample = GeoPoint64::new(view.target_lat + offset_deg, view.target_lon - offset_deg);
                let screen = proj
                    .project_ecef(geo_to_ecef(sample.lat, sample.lon, 0.0, proj.radius_m))
                    .expect("projected");
                assert!(screen.front_facing, "mode={mode:?} range={range_m}");

                let recovered = proj
                    .screen_to_geo(screen.pos)
                    .expect("ray hit sphere");
                let reprojected = proj
                    .project_ecef(geo_to_ecef(recovered.lat, recovered.lon, 0.0, proj.radius_m))
                    .expect("reprojected");
                let px_err = screen.pos.distance(reprojected.pos);
                assert!(
                    px_err < 0.02,
                    "round-trip error {px_err} px for mode={mode:?} range={range_m}"
                );
            }
        }
    }

    #[test]
    fn perspective_and_orthographic_match_scale_at_target_center() {
        let rect = test_rect();
        let mut view = GlobeViewState::from_focus(GeoPoint {
            lat: 47.6062,
            lon: -122.3321,
        });
        view.range_m = 4_000.0;
        view.tilt = 0.0;
        view.heading = 0.0;

        view.projection = ProjectionMode::Perspective;
        let persp = MapProjection::from_view(&view, ActiveBody::Earth, rect, 1.0);
        view.projection = ProjectionMode::Orthographic;
        let ortho = MapProjection::from_view(&view, ActiveBody::Earth, rect, 1.0);

        assert!((persp.meters_per_point() - ortho.meters_per_point()).abs() < 1e-6);

        // Two points 50 m apart near center should project to virtually identical pixel spacing.
        let p0 = geo_to_ecef(47.6062, -122.3321, 0.0, persp.radius_m);
        let p1 = geo_to_ecef(47.6062 + 50.0 / 111_320.0, -122.3321, 0.0, persp.radius_m);
        let dp = persp.project_ecef(p0).unwrap().pos.distance(persp.project_ecef(p1).unwrap().pos);
        let do_ = ortho.project_ecef(p0).unwrap().pos.distance(ortho.project_ecef(p1).unwrap().pos);
        assert!(
            (dp - do_).abs() < 0.05,
            "persp={dp} px vs ortho={do_} px"
        );
    }

    #[test]
    fn rtc_f32_shader_projection_matches_f64_cpu_to_subpixel_at_50m_and_dateline() {
        let rect = test_rect();
        for mode in [ProjectionMode::Perspective, ProjectionMode::Orthographic] {
            let mut view = GlobeViewState::from_focus(GeoPoint {
                lat: 40.0,
                lon: 179.9995,
            });
            view.target_elev_m = 180.0;
            view.range_m = 50.0;
            view.heading = 1.1;
            view.tilt = 0.85;
            view.projection = mode;
            view.local_layer_spread = 2.0;

            let proj = MapProjection::from_view(&view, ActiveBody::Earth, rect, 2.0);
            let tile_origin = geo_to_ecef(40.0, 179.9995, 0.0, proj.radius_m);
            let u = proj.rtc_uniforms(tile_origin);

            // Point ~10 metres away across the antimeridian at 185.5 m elevation (5.5 m above target).
            let pt_lat = 40.0 + 8.0 / 111_320.0;
            let pt_lon = -179.9994;
            let elev_m = 185.5;
            let cpu = proj.project_geo(pt_lat, pt_lon, elev_m).expect("cpu proj");

            // Literal transcription of `unified_camera.wgsl`:
            let local_sea = ecef_to_tile_local(
                geo_to_ecef(pt_lat, pt_lon, 0.0, proj.radius_m),
                tile_origin,
            );
            let tile_normal = Vec3::from_slice(&u.tile_normal[0..3]);
            let normal = (tile_normal + local_sea * u.tile_normal[3]).normalize();
            let displaced = local_sea + normal * (elev_m as f32 * u.depth_ctrl[1]);
            let rel = Vec3::from_slice(&u.rel_origin[0..3]) + displaced;
            let cx = rel.dot(Vec3::from_slice(&u.cam_right[0..3]));
            let cy = rel.dot(Vec3::from_slice(&u.cam_up[0..3]));
            let cz = rel.dot(Vec3::from_slice(&u.cam_forward[0..3]));
            let clip = if u.proj[3] > 0.5 {
                let nz = (u.proj[2] - cz) * u.depth_ctrl[0];
                glam::Vec4::new(cx * u.proj[0], cy * u.proj[1], nz, 1.0)
            } else {
                glam::Vec4::new(cx * u.proj[0], cy * u.proj[1], u.proj[2], cz)
            };
            let ndc = clip.truncate() / clip.w;
            let gpu_pos = egui::pos2(
                rect.left() + (ndc.x * 0.5 + 0.5) * rect.width(),
                rect.top() + (0.5 - ndc.y * 0.5) * rect.height(),
            );
            let err = cpu.pos.distance(gpu_pos);
            assert!(
                err < 0.01,
                "RTC WGSL vs f64 CPU error {err} px in mode={mode:?}"
            );
            assert!(
                (cpu.depth_ndc - ndc.z).abs() < 1e-4,
                "depth mismatch {} vs {}",
                cpu.depth_ndc,
                ndc.z
            );
        }
    }

    #[test]
    fn grab_pan_crosses_north_and_south_poles_smoothly() {
        let rect = test_rect();
        for sign in [-1.0_f64, 1.0_f64] {
            let mut view = GlobeViewState::from_focus(GeoPoint {
                lat: (sign * 89.92) as f32,
                lon: 170.0,
            });
            view.target_lat = sign * 89.92;
            view.target_lon = 170.0;
            view.range_m = 40_000.0;
            view.heading = 0.0;
            view.tilt = 0.4;

            let proj = MapProjection::from_view(&view, ActiveBody::Earth, rect, 1.0);
            // Grab a point near the pole and drag across the pole.
            let pole_screen = proj
                .project_ecef(geo_to_ecef(sign * 90.0, 0.0, 0.0, proj.radius_m))
                .expect("pole visible")
                .pos;
            let anchor = proj.screen_to_geo(pole_screen).expect("anchor");
            let dest = pole_screen + egui::vec2(12.0, 48.0 * sign as f32);

            let (new_geo, new_heading) = proj.grab_pan_step(pole_screen, dest).expect("pan");
            view.target_lat = new_geo.lat;
            view.target_lon = new_geo.lon;
            view.heading = new_heading;

            let proj_after = MapProjection::from_view(&view, ActiveBody::Earth, rect, 1.0);
            let moved = proj_after
                .project_ecef(geo_to_ecef(anchor.lat, anchor.lon, 0.0, proj_after.radius_m))
                .expect("after")
                .pos;
            let err = moved.distance(dest);
            assert!(err < 0.5, "pole grab-pan drift {err} px (sign={sign})");
        }
    }
}
