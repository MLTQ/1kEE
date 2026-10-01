use std::path::Path;

use crate::model::{ActiveBody, EventRecord, GeoPoint};
use crate::theme;

use super::super::marker_style::{MapMarker, draw_beam};
use super::super::srtm_stream;
use super::ProjectedLocalPoint;

pub(super) const EVENT_BEAM_HEIGHT: f32 = 110.0;
pub(super) const CAMERA_BEAM_HEIGHT: f32 = 76.0;

/// Small visual clearance that prevents glyph strokes from z-fighting with the
/// terrain mesh while keeping their perceived ground contact on the surface.
pub(super) const MARKER_SURFACE_CLEARANCE_M: f32 = 18.0;

pub(super) fn draw_camera_links(
    painter: &egui::Painter,
    event_marker: Option<egui::Pos2>,
    camera_markers: &[MapMarker],
) {
    let Some(event_marker) = event_marker else {
        return;
    };

    for marker in camera_markers {
        painter.line_segment(
            [event_marker, marker.base],
            egui::Stroke::new(0.75, theme::camera_color().gamma_multiply(0.32)),
        );
    }
}

/// Draw a Factal event as a glowing laser beam tapering to a point.
/// Identical visual treatment to globe_scene::draw_event_marker.
pub(super) fn draw_event_marker(
    painter: &egui::Painter,
    ground: ProjectedLocalPoint,
    tip: egui::Pos2,
    event: &EventRecord,
    is_selected: bool,
    time: f64,
    scale: f32,
) {
    let col = event.severity.color();
    draw_beam(painter, ground.pos, tip, col, 1.0, scale);

    // ── Ground strike ─────────────────────────────────────────────────────────
    if is_selected {
        let pulse = 9.0 + ((time as f32 * 2.6).sin() + 1.0) * 3.2;
        painter.circle_stroke(
            ground.pos,
            pulse * scale,
            egui::Stroke::new(1.3 * scale, theme::marker_glow_warm()),
        );
    }
    painter.circle_stroke(
        ground.pos,
        5.5 * scale,
        egui::Stroke::new(3.5 * scale, col.gamma_multiply(0.10)),
    );
    painter.circle_stroke(
        ground.pos,
        4.8 * scale,
        egui::Stroke::new(1.1 * scale, col.gamma_multiply(0.60)),
    );
    painter.circle_filled(ground.pos, 2.2 * scale, col);
}

/// Returns a marker anchor height above the terrain that is visibly rendered
/// this frame. The local scene supplies `displayed_surface_elevation_m` from
/// its fill mesh; raw SRTM remains an Earth-only nonblocking fallback for
/// contour-only frames.
pub(super) fn marker_surface_elevation_m(
    active_body: ActiveBody,
    selected_root: Option<&Path>,
    point: GeoPoint,
    displayed_surface_elevation_m: Option<f32>,
) -> f32 {
    // Marker paint must never be the first caller that decodes a full SRTM
    // raster or runs gdal_translate. Keep the existing fallback for the short
    // preload window, then repaint at the exact elevation once it is cached.
    let terrain_elevation_m = displayed_surface_elevation_m.unwrap_or_else(|| match active_body {
        ActiveBody::Earth => {
            srtm_stream::peek_elevation_m(selected_root, point).unwrap_or_else(|| {
                srtm_stream::request_elevation_preload(selected_root, point);
                0.0
            })
        }
        ActiveBody::Moon | ActiveBody::Mars => 0.0,
    });
    terrain_elevation_m + MARKER_SURFACE_CLEARANCE_M
}
