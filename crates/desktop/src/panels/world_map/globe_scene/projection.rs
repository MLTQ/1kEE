use crate::model::{GeoPoint, GlobeViewState};

use super::terrain_field;
use super::{GlobeLayout, ProjectedPoint};

/// Like `project_geo` but adds `extra_radius` (in globe-unit fractions) on
/// top of the terrain-based elevation.  Used to project a beam-tip point
/// directly above a geographic location so that the resulting screen-space
/// vector gives a perspective-correct beam direction: very short when the
/// event faces the camera, full-length when it is on the limb.
pub(super) fn project_geo_elevated(
    layout: &GlobeLayout,
    view: &GlobeViewState,
    point: GeoPoint,
    altitude_scale: f32,
    extra_radius: f32,
) -> Option<ProjectedPoint> {
    let lat = point.lat.to_radians();
    let lon = point.lon.to_radians();
    let elevation_signal = terrain_field::elevation(point) / 1.6;
    let signed_elevation = elevation_signal.mul_add(2.0, -1.0);
    let elevation = signed_elevation * altitude_scale;
    let radius = (1.0 + elevation + extra_radius).max(0.82);

    let mut x = radius * lat.cos() * lon.cos();
    let mut y = radius * lat.sin();
    let mut z = radius * lat.cos() * lon.sin();

    let yaw_cos = view.yaw.cos();
    let yaw_sin = view.yaw.sin();
    let x_yaw = x * yaw_cos + z * yaw_sin;
    let z_yaw = -x * yaw_sin + z * yaw_cos;
    x = x_yaw;
    z = z_yaw;

    let pitch_cos = view.pitch.cos();
    let pitch_sin = view.pitch.sin();
    let y_pitch = y * pitch_cos - z * pitch_sin;
    let z_pitch = y * pitch_sin + z * pitch_cos;
    y = y_pitch;
    z = z_pitch;

    let depth = layout.camera_distance - z;
    if depth <= 0.05 {
        return None;
    }

    let perspective = (layout.radius * layout.focal_length) / depth;
    let pos = egui::pos2(
        layout.center.x - x * perspective,
        layout.center.y - y * perspective,
    );

    Some(ProjectedPoint {
        pos,
        depth: ((z + 1.0) * 0.5).clamp(0.0, 1.0),
        front_facing: z >= 0.0,
    })
}

pub fn project_geo(
    layout: &GlobeLayout,
    view: &GlobeViewState,
    point: GeoPoint,
    altitude_scale: f32,
) -> Option<ProjectedPoint> {
    let lat = point.lat.to_radians();
    let lon = point.lon.to_radians();
    let elevation_signal = terrain_field::elevation(point) / 1.6;
    let signed_elevation = elevation_signal.mul_add(2.0, -1.0);
    let elevation = signed_elevation * altitude_scale;
    let radius = (1.0 + elevation).max(0.82);

    let mut x = radius * lat.cos() * lon.cos();
    let mut y = radius * lat.sin();
    let mut z = radius * lat.cos() * lon.sin();

    let yaw_cos = view.yaw.cos();
    let yaw_sin = view.yaw.sin();
    let x_yaw = x * yaw_cos + z * yaw_sin;
    let z_yaw = -x * yaw_sin + z * yaw_cos;
    x = x_yaw;
    z = z_yaw;

    let pitch_cos = view.pitch.cos();
    let pitch_sin = view.pitch.sin();
    let y_pitch = y * pitch_cos - z * pitch_sin;
    let z_pitch = y * pitch_sin + z * pitch_cos;
    y = y_pitch;
    z = z_pitch;

    let depth = layout.camera_distance - z;
    if depth <= 0.05 {
        return None;
    }

    let perspective = (layout.radius * layout.focal_length) / depth;
    let pos = egui::pos2(
        layout.center.x - x * perspective,
        layout.center.y - y * perspective,
    );

    Some(ProjectedPoint {
        pos,
        depth: ((z + 1.0) * 0.5).clamp(0.0, 1.0),
        front_facing: z >= 0.0,
    })
}

/// Like `project_geo` but skips `terrain_field::elevation` — uses a constant
/// radius offset instead.  Eliminates 6 `exp()` calls per point; the ±1.5%
/// terrain-driven radius variation is imperceptible on thin line strokes.
fn project_geo_flat(
    layout: &GlobeLayout,
    view: &GlobeViewState,
    point: GeoPoint,
    radius_offset: f32,
) -> Option<ProjectedPoint> {
    let lat = point.lat.to_radians();
    let lon = point.lon.to_radians();
    let radius = 1.0_f32 + radius_offset;

    let mut x = radius * lat.cos() * lon.cos();
    let mut y = radius * lat.sin();
    let mut z = radius * lat.cos() * lon.sin();

    let yaw_cos = view.yaw.cos();
    let yaw_sin = view.yaw.sin();
    let x_yaw = x * yaw_cos + z * yaw_sin;
    let z_yaw = -x * yaw_sin + z * yaw_cos;
    x = x_yaw;
    z = z_yaw;

    let pitch_cos = view.pitch.cos();
    let pitch_sin = view.pitch.sin();
    let y_pitch = y * pitch_cos - z * pitch_sin;
    let z_pitch = y * pitch_sin + z * pitch_cos;
    y = y_pitch;
    z = z_pitch;

    let depth = layout.camera_distance - z;
    if depth <= 0.05 {
        return None;
    }

    let perspective = (layout.radius * layout.focal_length) / depth;
    let pos = egui::pos2(
        layout.center.x - x * perspective,
        layout.center.y - y * perspective,
    );

    Some(ProjectedPoint {
        pos,
        depth: ((z + 1.0) * 0.5).clamp(0.0, 1.0),
        front_facing: z >= 0.0,
    })
}

/// Project a geographic polyline to screen-space segments, splitting at the
/// horizon, without touching the painter.  Returns a list of continuous
/// visible segments (each ≥ 2 points).  Used by parallel projection paths.
pub(super) fn project_path_segments(
    layout: &GlobeLayout,
    view: &GlobeViewState,
    path: &[GeoPoint],
    altitude_scale: f32,
) -> Vec<Vec<egui::Pos2>> {
    let mut segments: Vec<Vec<egui::Pos2>> = Vec::new();
    let mut current: Vec<egui::Pos2> = Vec::new();

    for point in path {
        match project_geo_flat(layout, view, *point, altitude_scale) {
            Some(p) if p.front_facing => current.push(p.pos),
            _ => {
                if current.len() >= 2 {
                    segments.push(std::mem::take(&mut current));
                } else {
                    current.clear();
                }
            }
        }
    }
    if current.len() >= 2 {
        segments.push(current);
    }
    segments
}

/// Draw a geographic polyline on the globe, clipping at the horizon.
///
/// Uses a flat (constant-radius) projection — no terrain field — for
/// performance. Back-facing segments are skipped entirely (they are
/// nearly invisible at the alpha values used and were the source of
/// "laser" artifacts when single orphan points straddled the horizon).
pub(super) fn draw_geo_path(
    painter: &egui::Painter,
    layout: &GlobeLayout,
    view: &GlobeViewState,
    path: &[GeoPoint],
    altitude_scale: f32,
    front_color: egui::Color32,
    _backface_alpha: f32,
) {
    let stroke = egui::Stroke::new(1.15, front_color.gamma_multiply(0.92));
    let mut segment: Vec<egui::Pos2> = Vec::new();

    for point in path {
        match project_geo_flat(layout, view, *point, altitude_scale) {
            Some(p) if p.front_facing => segment.push(p.pos),
            _ => {
                // Back-facing or behind near-plane — break the current segment.
                // Always clear (even a single-point orphan) to prevent the orphan
                // being joined to the next visible run, which produced "laser" lines.
                if segment.len() >= 2 {
                    painter.add(egui::Shape::line(std::mem::take(&mut segment), stroke));
                } else {
                    segment.clear();
                }
            }
        }
    }

    if segment.len() >= 2 {
        painter.add(egui::Shape::line(segment, stroke));
    }
}
