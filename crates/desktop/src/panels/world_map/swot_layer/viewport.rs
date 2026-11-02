//! Choose a native image window from the visible globe, including limb and poles.
use super::super::globe_scene::{GlobeLayout, project_surface, screen_to_latlon};
use super::grid::{OVERVIEW, Region};
use crate::model::{GeoPoint, GlobeViewState};

pub(super) fn detail_region(
    rect: egui::Rect,
    layout: &GlobeLayout,
    view: &GlobeViewState,
    pixels_per_point: f32,
) -> Option<Region> {
    // Refine when a preview texel would be larger than a physical screen pixel
    // near the view center. This responds to zoom, window size and display scale.
    let scale = layout.radius * layout.focal_length / (layout.camera_distance - 1.0);
    if scale * std::f32::consts::TAU / OVERVIEW[0] as f32 * pixels_per_point <= 1.0 {
        return None;
    }
    let center = view.globe_center_latlon();
    let mut bounds = [
        f32::INFINITY,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::NEG_INFINITY,
    ];
    let mut include = |position| {
        if let Some(point) = screen_to_latlon(rect, view, position) {
            let lon = center.lon + (point.lon - center.lon + 180.0).rem_euclid(360.0) - 180.0;
            bounds[0] = bounds[0].min(lon);
            bounds[1] = bounds[1].min(point.lat);
            bounds[2] = bounds[2].max(lon);
            bounds[3] = bounds[3].max(point.lat);
        }
    };
    // Interior and border samples describe zoomed views that cut the sphere.
    for row in 0..=16 {
        for col in 0..=16 {
            include(egui::pos2(
                rect.left() + rect.width() * col as f32 / 16.0,
                rect.top() + rect.height() * row as f32 / 16.0,
            ));
        }
    }
    // If the limb is in the canvas, the screen rectangle's corners miss Earth.
    let radius = layout.radius * layout.focal_length
        / (layout.camera_distance.powi(2) - 1.0).sqrt()
        * 0.9999;
    for i in 0..128 {
        let angle = i as f32 * std::f32::consts::TAU / 128.0;
        let position = layout.center + egui::vec2(angle.cos(), angle.sin()) * radius;
        if rect.contains(position) {
            include(position);
        }
    }
    if !bounds.iter().all(|v| v.is_finite()) {
        return None;
    }
    let margin_lon = ((bounds[2] - bounds[0]) * 0.08).max(1.0);
    let margin_lat = ((bounds[3] - bounds[1]) * 0.08).max(1.0);
    bounds = [
        bounds[0] - margin_lon,
        bounds[1] - margin_lat,
        bounds[2] + margin_lon,
        bounds[3] + margin_lat,
    ];
    for lat in [-90.0, 90.0] {
        if project_surface(
            layout,
            view,
            GeoPoint {
                lat,
                lon: center.lon,
            },
        )
        .is_some_and(|p| {
            p.depth * 2.0 - 1.0 >= 1.0 / layout.camera_distance && rect.contains(p.pos)
        }) {
            bounds[0] = -180.0;
            bounds[2] = 180.0;
            if lat > 0.0 {
                bounds[3] = 90.0;
            } else {
                bounds[1] = -90.0;
            }
        }
    }
    Some(Region::from_bounds(
        bounds[0], bounds[1], bounds[2], bounds[3],
    ))
}

#[cfg(test)]
#[path = "viewport_tests.rs"]
mod tests;
