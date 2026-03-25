use crate::model::{GeoPoint, GlobeViewState};
use crate::osm_ingest::GeoBounds as OsmGeoBounds;

use super::super::srtm_focus_cache;
use super::{BASE_VERTICAL_EXAGGERATION, LocalLayout, ProjectedLocalPoint};

pub(super) fn visual_half_extent_for_zoom_inner(view_zoom: f32) -> f32 {
    super::visual_half_extent_for_zoom(view_zoom)
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
    let x_km = (point.lon - focus.lon) * 111.32 * focus.lat.to_radians().cos().abs().max(0.2);
    // Standard orientation: positive y_km = north.  North is mapped upward on screen by
    // negating the ground_y_pitch / ground_z_pitch terms in the screen-y formula below.
    let y_km = (point.lat - focus.lat) * 111.32;

    let x = x_km / extent_x_km;
    let y = y_km / extent_y_km;
    // Normalize elevation against the current terrain span so vertical relief
    // scales with zoom instead of being added as a fixed screen-space offset.
    // Without this, horizontal distances expand/contract with zoom while
    // elevation stays effectively constant in pixels, which makes mountains
    // look wildly taller or flatter depending on zoom.
    let reference_span_km = ((extent_x_km + extent_y_km) * 0.5).max(1.0);
    let z = (elevation_m / 1000.0) * BASE_VERTICAL_EXAGGERATION / reference_span_km;

    let yaw_cos = view.local_yaw.cos();
    let yaw_sin = view.local_yaw.sin();
    let x_yaw = x * yaw_cos - y * yaw_sin;
    let y_yaw = x * yaw_sin + y * yaw_cos;

    let pitch_cos = view.local_pitch.cos();
    let pitch_sin = view.local_pitch.sin();
    let ground_y_pitch = y_yaw * pitch_cos;
    let ground_z_pitch = y_yaw * pitch_sin;
    let elevation_y_offset = z * pitch_sin;
    let elevation_z_offset = z * pitch_cos;
    let z_pitch = ground_z_pitch + elevation_z_offset;

    let ground_pitch_scale = layout.height * 0.55;
    // Keep in sync with shader gds constant: must be < gps/tan(max_pitch).
    // max_pitch=1.55 rad → tan≈48 → threshold ≈0.0114.  Using 0.01 for safety.
    let ground_depth_scale = layout.height * 0.01;
    let elevation_pitch_scale = layout.height * 0.55 * view.local_layer_spread;
    let elevation_depth_scale = layout.height * 0.24 * view.local_layer_spread;

    let pos = egui::pos2(
        layout.focus_center.x + x_yaw * layout.horizontal_scale,
        // Negate the ground terms so that positive y_yaw (north) moves upward on screen.
        // Elevation terms are unchanged: positive elevation still lifts features upward.
        layout.focus_center.y - ground_y_pitch * ground_pitch_scale
            + ground_z_pitch * ground_depth_scale
            - elevation_y_offset * elevation_pitch_scale
            - elevation_z_offset * elevation_depth_scale,
    );

    // Let egui's painter clip rect cull off-screen geometry; only reject points
    // that are wildly out of range (NaN / extreme float blown projections).
    (pos.x.is_finite()
        && pos.y.is_finite()
        && pos.x >= layout.center.x - layout.width * 4.0
        && pos.x <= layout.center.x + layout.width * 4.0
        && pos.y >= layout.center.y - layout.height * 4.0
        && pos.y <= layout.center.y + layout.height * 4.0)
        .then_some(ProjectedLocalPoint {
            pos,
            depth: (1.0 + z_pitch).clamp(0.0, 1.0),
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
