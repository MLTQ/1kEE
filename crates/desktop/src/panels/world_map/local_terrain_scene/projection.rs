use crate::model::{GeoPoint, GlobeViewState};
use crate::osm_ingest::GeoBounds as OsmGeoBounds;

use super::super::srtm_focus_cache;
use super::{BASE_VERTICAL_EXAGGERATION, LocalLayout, ProjectedLocalPoint};

pub(super) fn visual_half_extent_for_zoom_inner(view_zoom: f32) -> f32 {
    super::visual_half_extent_for_zoom(view_zoom)
}

/// All inputs to `project_local` that are invariant across the points of a frame.
/// Used as a cache key so the per-point trig/scale setup is computed once, not
/// once per point (there are hundreds of thousands of points per frame).
#[derive(Clone, Copy, PartialEq)]
struct ProjKey {
    focus_lat: f32,
    focus_lon: f32,
    local_yaw: f32,
    local_pitch: f32,
    local_layer_spread: f32,
    extent_x_km: f32,
    extent_y_km: f32,
    focus_center_x: f32,
    focus_center_y: f32,
    height: f32,
    horizontal_scale: f32,
    center_x: f32,
    center_y: f32,
    width: f32,
}

/// The scale and rotation factors `project` applies, derived once per frame.
///
/// This is the single definition of the local oblique transform. The CPU
/// projector below and `local_contour_pass`'s vertex shader both consume it, so
/// GPU-drawn contours land on exactly the pixels CPU-drawn markers and roads do.
/// `local_projection_matches_the_shader` pins the arithmetic.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LocalProjectionParams {
    pub focus_lat: f32,
    pub focus_lon: f32,
    /// `(lon - focus_lon) * x_factor` → normalized, extent-scaled x.
    pub x_factor: f32,
    /// `(lat - focus_lat) * y_factor` → normalized, extent-scaled y.
    pub y_factor: f32,
    /// `elevation_m * z_factor` → normalized z.
    pub z_factor: f32,
    pub yaw_cos: f32,
    pub yaw_sin: f32,
    pub pitch_cos: f32,
    pub pitch_sin: f32,
    pub focus_center_x: f32,
    pub focus_center_y: f32,
    pub horizontal_scale: f32,
    pub ground_pitch_scale: f32,
    pub ground_depth_scale: f32,
    pub elevation_pitch_scale: f32,
    pub elevation_depth_scale: f32,
}

impl LocalProjectionParams {
    fn from_key(key: ProjKey) -> Self {
        let cos_clamp = key.focus_lat.to_radians().cos().abs().max(0.2);
        let reference_span_km = ((key.extent_x_km + key.extent_y_km) * 0.5).max(1.0);
        Self {
            focus_lat: key.focus_lat,
            focus_lon: key.focus_lon,
            x_factor: 111.32 * cos_clamp / key.extent_x_km,
            y_factor: 111.32 / key.extent_y_km,
            z_factor: BASE_VERTICAL_EXAGGERATION / (1000.0 * reference_span_km),
            yaw_cos: key.local_yaw.cos(),
            yaw_sin: key.local_yaw.sin(),
            pitch_cos: key.local_pitch.cos(),
            pitch_sin: key.local_pitch.sin(),
            focus_center_x: key.focus_center_x,
            focus_center_y: key.focus_center_y,
            horizontal_scale: key.horizontal_scale,
            ground_pitch_scale: key.height * 0.55,
            // Must stay below `ground_pitch_scale / tan(max_pitch)`:
            // max_pitch = 1.55 rad → tan ≈ 48 → threshold ≈ 0.0114. 0.01 for safety.
            ground_depth_scale: key.height * 0.01,
            elevation_pitch_scale: key.height * 0.55 * key.local_layer_spread,
            elevation_depth_scale: key.height * 0.24 * key.local_layer_spread,
        }
    }

    /// Screen position in logical points, without the CPU projector's
    /// out-of-range rejection. The shader's `project` is this function.
    #[inline]
    pub(crate) fn project_logical(&self, point: GeoPoint, elevation_m: f32) -> (f32, f32, f32) {
        // Standard orientation: positive y = north, mapped upward on screen by
        // negating the ground terms in the screen-y formula below.
        let x = (point.lon - self.focus_lon) * self.x_factor;
        let y = (point.lat - self.focus_lat) * self.y_factor;
        let z = elevation_m * self.z_factor;

        let x_yaw = x * self.yaw_cos - y * self.yaw_sin;
        let y_yaw = x * self.yaw_sin + y * self.yaw_cos;

        let ground_y_pitch = y_yaw * self.pitch_cos;
        let ground_z_pitch = y_yaw * self.pitch_sin;
        let elevation_y_offset = z * self.pitch_sin;
        let elevation_z_offset = z * self.pitch_cos;

        (
            self.focus_center_x + x_yaw * self.horizontal_scale,
            self.focus_center_y - ground_y_pitch * self.ground_pitch_scale
                + ground_z_pitch * self.ground_depth_scale
                - elevation_y_offset * self.elevation_pitch_scale
                - elevation_z_offset * self.elevation_depth_scale,
            ground_z_pitch + elevation_z_offset,
        )
    }
}

/// Frame-invariant projection constants, derived once per `ProjKey`. `project`
/// then does only the cheap per-point arithmetic.
struct LocalProjector {
    key: ProjKey,
    params: LocalProjectionParams,
    min_x: f32,
    max_x: f32,
    min_y: f32,
    max_y: f32,
}

impl LocalProjector {
    fn new(key: ProjKey) -> Self {
        LocalProjector {
            key,
            params: LocalProjectionParams::from_key(key),
            min_x: key.center_x - key.width * 4.0,
            max_x: key.center_x + key.width * 4.0,
            min_y: key.center_y - key.height * 4.0,
            max_y: key.center_y + key.height * 4.0,
        }
    }

    #[inline]
    fn project(&self, point: GeoPoint, elevation_m: f32) -> Option<ProjectedLocalPoint> {
        let (pos_x, pos_y, z_pitch) = self.params.project_logical(point, elevation_m);
        let pos = egui::pos2(pos_x, pos_y);

        // Let egui's painter clip rect cull off-screen geometry; only reject points
        // that are wildly out of range (NaN / extreme float blown projections).
        (pos.x.is_finite()
            && pos.y.is_finite()
            && pos.x >= self.min_x
            && pos.x <= self.max_x
            && pos.y >= self.min_y
            && pos.y <= self.max_y)
            .then_some(ProjectedLocalPoint {
                pos,
                depth: (1.0 + z_pitch).clamp(0.0, 1.0),
            })
    }
}

/// The frame's projection parameters, for callers that project somewhere other
/// than the CPU — currently `local_contour_pass`, which feeds them to a vertex
/// shader. Built from the same `ProjKey` as `project_local`, so both paths are
/// guaranteed to agree.
pub(crate) fn local_projection_params(
    layout: &LocalLayout,
    view: &GlobeViewState,
    focus: GeoPoint,
    extent_x_km: f32,
    extent_y_km: f32,
) -> LocalProjectionParams {
    LocalProjectionParams::from_key(proj_key(
        layout,
        view,
        focus,
        extent_x_km,
        extent_y_km,
    ))
}

fn proj_key(
    layout: &LocalLayout,
    view: &GlobeViewState,
    focus: GeoPoint,
    extent_x_km: f32,
    extent_y_km: f32,
) -> ProjKey {
    ProjKey {
        focus_lat: focus.lat,
        focus_lon: focus.lon,
        local_yaw: view.local_yaw,
        local_pitch: view.local_pitch,
        local_layer_spread: view.local_layer_spread,
        extent_x_km,
        extent_y_km,
        focus_center_x: layout.focus_center.x,
        focus_center_y: layout.focus_center.y,
        height: layout.height,
        horizontal_scale: layout.horizontal_scale,
        center_x: layout.center.x,
        center_y: layout.center.y,
        width: layout.width,
    }
}

pub(super) fn project_local(
    layout: &LocalLayout,
    view: &GlobeViewState,
    focus: GeoPoint,
    point: GeoPoint,
    elevation_m: f32,
    extent_x_km: f32,
    extent_y_km: f32,
) -> Option<ProjectedLocalPoint> {
    let key = proj_key(layout, view, focus, extent_x_km, extent_y_km);

    // The frame-invariant setup (5 transcendentals + a dozen scale factors) is
    // identical for every point of a frame, so cache it per thread and recompute
    // only when the view/layout changes. Thread-local keeps the parallel (rayon)
    // contour projection lock-free — each worker memoizes independently.
    thread_local! {
        static CACHE: std::cell::RefCell<Option<LocalProjector>> = const { std::cell::RefCell::new(None) };
    }
    CACHE.with(|c| {
        let mut slot = c.borrow_mut();
        if slot.as_ref().map(|p| p.key) != Some(key) {
            *slot = Some(LocalProjector::new(key));
        }
        slot.as_ref().unwrap().project(point, elevation_m)
    })
}

pub(super) fn local_geo_bounds(center: GeoPoint, view_zoom: f32) -> OsmGeoBounds {
    let half_extent_deg = super::visual_half_extent_for_zoom(view_zoom);
    OsmGeoBounds {
        min_lat: (center.lat - half_extent_deg).clamp(-85.0511, 85.0511),
        max_lat: (center.lat + half_extent_deg).clamp(-85.0511, 85.0511),
        min_lon: (center.lon - half_extent_deg).clamp(-180.0, 180.0),
        max_lon: (center.lon + half_extent_deg).clamp(-180.0, 180.0),
    }
}

pub(super) fn road_tile_zoom(render_zoom: f32) -> u8 {
    if render_zoom >= 10.0 {
        10
    } else if render_zoom >= 6.0 {
        8
    } else if render_zoom >= 3.5 {
        6
    } else {
        4
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The vertex shader in `local_contour_lines.wgsl` reimplements
    /// `LocalProjectionParams::project_logical`. Nothing in the build checks
    /// that the two agree — a mismatch would misplace every GPU-drawn contour
    /// against the CPU-drawn markers and roads beside it — so this pins the
    /// scalar arithmetic the shader was transcribed from.
    ///
    /// If this test is edited, edit `project` in the shader to match.
    #[test]
    fn local_projection_matches_the_shader() {
        let params = LocalProjectionParams {
            focus_lat: 40.0,
            focus_lon: -105.0,
            x_factor: 3.0,
            y_factor: 5.0,
            z_factor: 0.002,
            yaw_cos: 0.8,
            yaw_sin: 0.6,
            pitch_cos: 0.6,
            pitch_sin: 0.8,
            focus_center_x: 400.0,
            focus_center_y: 300.0,
            horizontal_scale: 2.0,
            ground_pitch_scale: 220.0,
            ground_depth_scale: 4.0,
            elevation_pitch_scale: 110.0,
            elevation_depth_scale: 48.0,
        };
        let point = GeoPoint {
            lat: 40.1,
            lon: -104.9,
        };
        let elevation_m = 1_500.0;

        // Transcription of the shader body, kept deliberately literal.
        let x = (point.lon - params.focus_lon) * params.x_factor;
        let y = (point.lat - params.focus_lat) * params.y_factor;
        let z = elevation_m * params.z_factor;
        let x_yaw = x * params.yaw_cos - y * params.yaw_sin;
        let y_yaw = x * params.yaw_sin + y * params.yaw_cos;
        let ground_y_pitch = y_yaw * params.pitch_cos;
        let ground_z_pitch = y_yaw * params.pitch_sin;
        let elevation_y_offset = z * params.pitch_sin;
        let elevation_z_offset = z * params.pitch_cos;
        let expected_x = params.focus_center_x + x_yaw * params.horizontal_scale;
        let expected_y = params.focus_center_y
            - ground_y_pitch * params.ground_pitch_scale
            + ground_z_pitch * params.ground_depth_scale
            - elevation_y_offset * params.elevation_pitch_scale
            - elevation_z_offset * params.elevation_depth_scale;

        let (got_x, got_y, got_depth) = params.project_logical(point, elevation_m);
        assert!((got_x - expected_x).abs() < 1e-3, "x: {got_x} vs {expected_x}");
        assert!((got_y - expected_y).abs() < 1e-3, "y: {got_y} vs {expected_y}");
        assert!((got_depth - (ground_z_pitch + elevation_z_offset)).abs() < 1e-3);
    }

    /// The CPU projector must be exactly the shared params plus its range
    /// rejection, or the two renderers drift apart silently.
    #[test]
    fn the_cpu_projector_uses_the_shared_parameters() {
        let key = ProjKey {
            focus_lat: 40.0,
            focus_lon: -105.0,
            local_yaw: 0.3,
            local_pitch: 0.9,
            local_layer_spread: 1.0,
            extent_x_km: 3.0,
            extent_y_km: 4.0,
            focus_center_x: 400.0,
            focus_center_y: 300.0,
            height: 600.0,
            horizontal_scale: 2.0,
            center_x: 400.0,
            center_y: 300.0,
            width: 800.0,
        };
        let projector = LocalProjector::new(key);
        let point = GeoPoint {
            lat: 40.01,
            lon: -104.99,
        };
        let projected = projector.project(point, 900.0).expect("in range");
        let (x, y, _) = LocalProjectionParams::from_key(key).project_logical(point, 900.0);
        assert!((projected.pos.x - x).abs() < 1e-4);
        assert!((projected.pos.y - y).abs() < 1e-4);
    }
}

