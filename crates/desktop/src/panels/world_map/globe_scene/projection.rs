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

pub(super) fn draw_geo_path(
    painter: &egui::Painter,
    layout: &GlobeLayout,
    view: &GlobeViewState,
    path: &[GeoPoint],
    altitude_scale: f32,
    front_color: egui::Color32,
    backface_alpha: f32,
) {
    let mut front_segment = Vec::new();
    let mut back_segment = Vec::new();

    for point in path {
        if let Some(projected) = project_geo(layout, view, *point, altitude_scale) {
            if projected.front_facing {
                if back_segment.len() >= 2 {
                    painter.add(egui::Shape::line(
                        std::mem::take(&mut back_segment),
                        egui::Stroke::new(0.55, front_color.gamma_multiply(backface_alpha)),
                    ));
                }
                front_segment.push(projected.pos);
            } else {
                if front_segment.len() >= 2 {
                    painter.add(egui::Shape::line(
                        std::mem::take(&mut front_segment),
                        egui::Stroke::new(1.15, front_color.gamma_multiply(0.88)),
                    ));
                }
                back_segment.push(projected.pos);
            }
        } else {
            flush_segments(
                painter,
                &mut front_segment,
                &mut back_segment,
                front_color,
                backface_alpha,
            );
        }
    }

    flush_segments(
        painter,
        &mut front_segment,
        &mut back_segment,
        front_color,
        backface_alpha,
    );
}

pub(super) fn flush_segments(
    painter: &egui::Painter,
    front_segment: &mut Vec<egui::Pos2>,
    back_segment: &mut Vec<egui::Pos2>,
    front_color: egui::Color32,
    backface_alpha: f32,
) {
    if front_segment.len() >= 2 {
        painter.add(egui::Shape::line(
            std::mem::take(front_segment),
            egui::Stroke::new(0.95, front_color.gamma_multiply(0.92)),
        ));
    } else {
        front_segment.clear();
    }

    if back_segment.len() >= 2 {
        painter.add(egui::Shape::line(
            std::mem::take(back_segment),
            egui::Stroke::new(0.4, front_color.gamma_multiply(backface_alpha)),
        ));
    } else {
        back_segment.clear();
    }
}
