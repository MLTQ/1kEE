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

/// Frame-invariant projection constants, derived once per `ProjKey`. `project`
/// then does only the cheap per-point arithmetic.
struct LocalProjector {
    key: ProjKey,
    focus_lat: f32,
    focus_lon: f32,
    x_factor: f32, // (lon - focus.lon) * x_factor  → normalized, extent-scaled x
    y_factor: f32, // (lat - focus.lat) * y_factor  → normalized, extent-scaled y
    z_factor: f32, // elevation_m * z_factor         → normalized z
    yaw_cos: f32,
    yaw_sin: f32,
    pitch_cos: f32,
    pitch_sin: f32,
    focus_center: egui::Pos2,
    horizontal_scale: f32,
    ground_pitch_scale: f32,
    ground_depth_scale: f32,
    elevation_pitch_scale: f32,
    elevation_depth_scale: f32,
    min_x: f32,
    max_x: f32,
    min_y: f32,
    max_y: f32,
}

impl LocalProjector {
    fn new(key: ProjKey) -> Self {
        let cos_clamp = key.focus_lat.to_radians().cos().abs().max(0.2);
        let reference_span_km = ((key.extent_x_km + key.extent_y_km) * 0.5).max(1.0);
        LocalProjector {
            key,
            focus_lat: key.focus_lat,
            focus_lon: key.focus_lon,
            x_factor: 111.32 * cos_clamp / key.extent_x_km,
            y_factor: 111.32 / key.extent_y_km,
            z_factor: BASE_VERTICAL_EXAGGERATION / (1000.0 * reference_span_km),
            yaw_cos: key.local_yaw.cos(),
            yaw_sin: key.local_yaw.sin(),
            pitch_cos: key.local_pitch.cos(),
            pitch_sin: key.local_pitch.sin(),
            focus_center: egui::pos2(key.focus_center_x, key.focus_center_y),
            horizontal_scale: key.horizontal_scale,
            ground_pitch_scale: key.height * 0.55,
            // Keep in sync with shader gds constant: must be < gps/tan(max_pitch).
            // max_pitch=1.55 rad → tan≈48 → threshold ≈0.0114.  Using 0.01 for safety.
            ground_depth_scale: key.height * 0.01,
            elevation_pitch_scale: key.height * 0.55 * key.local_layer_spread,
            elevation_depth_scale: key.height * 0.24 * key.local_layer_spread,
            min_x: key.center_x - key.width * 4.0,
            max_x: key.center_x + key.width * 4.0,
            min_y: key.center_y - key.height * 4.0,
            max_y: key.center_y + key.height * 4.0,
        }
    }

    #[inline]
    fn project(&self, point: GeoPoint, elevation_m: f32) -> Option<ProjectedLocalPoint> {
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
        let z_pitch = ground_z_pitch + elevation_z_offset;

        let pos = egui::pos2(
            self.focus_center.x + x_yaw * self.horizontal_scale,
            self.focus_center.y - ground_y_pitch * self.ground_pitch_scale
                + ground_z_pitch * self.ground_depth_scale
                - elevation_y_offset * self.elevation_pitch_scale
                - elevation_z_offset * self.elevation_depth_scale,
        );

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

pub(super) fn project_local(
    layout: &LocalLayout,
    view: &GlobeViewState,
    focus: GeoPoint,
    point: GeoPoint,
    elevation_m: f32,
    extent_x_km: f32,
    extent_y_km: f32,
) -> Option<ProjectedLocalPoint> {
    let key = ProjKey {
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
    };

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
