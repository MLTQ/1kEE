use std::path::Path;

use crate::model::{EventRecord, GeoPoint, GlobeViewState, NearbyCamera};
use crate::theme;

use super::{LocalLayout, ProjectedLocalPoint, visual_half_extent_for_zoom};
use super::projection::project_local;
use super::super::srtm_stream;

/// Height in screen-space pixels of an event laser beam.
const EVENT_BEAM_HEIGHT_PX: f32 = 110.0;

#[allow(dead_code)]
pub(super) fn draw_markers(
    painter: &egui::Painter,
    layout: &LocalLayout,
    view: &GlobeViewState,
    selected_root: Option<&Path>,
    viewport_center: GeoPoint,
    _render_zoom: f32,
    event: &EventRecord,
    nearby: &[NearbyCamera],
    selected_event_id: Option<&str>,
    selected_camera_id: Option<&str>,
    time: f64,
) -> (Vec<(String, egui::Pos2)>, Vec<(String, egui::Pos2)>) {
    let half_extent_deg = visual_half_extent_for_zoom(view.local_zoom);
    let km_per_deg_lat = 111.32f32;
    let km_per_deg_lon = km_per_deg_lat * viewport_center.lat.to_radians().cos().abs().max(0.2);
    let extent_x_km = (half_extent_deg * km_per_deg_lon).max(1.0);
    let extent_y_km = (half_extent_deg * km_per_deg_lat).max(1.0);

    let event_elev = marker_elevation_m(selected_root, event.location);
    let event_marker = project_local(
        layout,
        view,
        viewport_center,
        event.location,
        event_elev,
        extent_x_km,
        extent_y_km,
    );
    // Compute the screen-space "up" direction by projecting the same point at
    // a higher elevation and measuring the displacement.  Normalising then
    // scaling gives a beam of consistent pixel length regardless of zoom.
    let event_sky = project_local(
        layout, view, viewport_center, event.location,
        event_elev + 1000.0, extent_x_km, extent_y_km,
    );
    if let Some(event_marker) = event_marker {
        let tip = event_sky.map(|sky| {
            let dx = sky.pos.x - event_marker.pos.x;
            let dy = sky.pos.y - event_marker.pos.y;
            let len = (dx * dx + dy * dy).sqrt().max(0.1);
            egui::pos2(
                event_marker.pos.x + dx / len * EVENT_BEAM_HEIGHT_PX,
                event_marker.pos.y + dy / len * EVENT_BEAM_HEIGHT_PX,
            )
        }).unwrap_or(egui::pos2(event_marker.pos.x, event_marker.pos.y - EVENT_BEAM_HEIGHT_PX));

        draw_event_marker(
            painter,
            event_marker,
            tip,
            event,
            selected_event_id == Some(event.id.as_str()),
            time,
        );
    }

    let camera_markers = nearby
        .iter()
        .filter_map(|camera| {
            project_local(
                layout,
                view,
                viewport_center,
                camera.location,
                marker_elevation_m(selected_root, camera.location),
                extent_x_km,
                extent_y_km,
            )
            .map(|projected| {
                draw_camera_marker(
                    painter,
                    projected,
                    selected_camera_id == Some(camera.id.as_str()),
                );
                (camera.id.clone(), projected.pos)
            })
        })
        .collect();

    (
        event_marker
            .map(|marker| vec![(event.id.clone(), marker.pos)])
            .unwrap_or_default(),
        camera_markers,
    )
}

pub(super) fn draw_camera_links(
    painter: &egui::Painter,
    event_marker: Option<egui::Pos2>,
    camera_markers: &[(String, egui::Pos2)],
) {
    let Some(event_marker) = event_marker else {
        return;
    };

    for (_, marker) in camera_markers {
        painter.line_segment(
            [event_marker, *marker],
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
) {
    let col = event.severity.color();
    let dx = tip.x - ground.pos.x;
    let dy = tip.y - ground.pos.y;

    // ── Atmospheric halos — taper in width and alpha toward the tip ───────────
    const HALO_SEGS: u32 = 7;
    for i in 0..HALO_SEGS {
        let t0 = i as f32 / HALO_SEGS as f32;
        let t1 = (i + 1) as f32 / HALO_SEGS as f32;
        let tm = (t0 + t1) * 0.5;
        let a = (1.0 - tm).powi(2);
        let p0 = egui::pos2(ground.pos.x + dx * t0, ground.pos.y + dy * t0);
        let p1 = egui::pos2(ground.pos.x + dx * t1, ground.pos.y + dy * t1);
        painter.line_segment([p0, p1], egui::Stroke::new((22.0 * a).max(0.5), col.gamma_multiply(0.04 * a)));
        painter.line_segment([p0, p1], egui::Stroke::new((11.0 * a).max(0.5), col.gamma_multiply(0.08 * a)));
        painter.line_segment([p0, p1], egui::Stroke::new(( 4.5 * a).max(0.5), col.gamma_multiply(0.16 * a)));
    }

    // ── Tapering core — cubic fade, width narrows to a point ─────────────────
    const SEGS: u32 = 14;
    for i in 0..SEGS {
        let t0 = i as f32 / SEGS as f32;
        let t1 = (i + 1) as f32 / SEGS as f32;
        let tm = (t0 + t1) * 0.5;
        let falloff = 1.0 - tm;
        let alpha   = falloff.powi(3);
        let w_glow  = (4.0 * falloff.powf(0.7)).max(0.4);
        let w_core  = (1.7 * falloff.powf(0.7)).max(0.3);
        let p0 = egui::pos2(ground.pos.x + dx * t0, ground.pos.y + dy * t0);
        let p1 = egui::pos2(ground.pos.x + dx * t1, ground.pos.y + dy * t1);
        painter.line_segment([p0, p1], egui::Stroke::new(w_glow, col.gamma_multiply(alpha * 0.30)));
        painter.line_segment([p0, p1], egui::Stroke::new(w_core, col.gamma_multiply(alpha * 0.96)));
    }

    // ── Ground strike ─────────────────────────────────────────────────────────
    if is_selected {
        let pulse = 9.0 + ((time as f32 * 2.6).sin() + 1.0) * 3.2;
        painter.circle_stroke(
            ground.pos, pulse,
            egui::Stroke::new(1.3, theme::marker_glow_warm()),
        );
    }
    painter.circle_stroke(ground.pos, 5.5, egui::Stroke::new(3.5, col.gamma_multiply(0.10)));
    painter.circle_stroke(ground.pos, 4.8, egui::Stroke::new(1.1, col.gamma_multiply(0.60)));
    painter.circle_filled(ground.pos, 2.2, col);
}

pub(super) fn draw_camera_marker(painter: &egui::Painter, marker: ProjectedLocalPoint, is_selected: bool) {
    let radius = 3.4 + marker.depth;
    let color = if is_selected { theme::marker_camera_ring() } else { theme::camera_color() };

    // Soft halo so cameras read against the terrain
    painter.circle_stroke(
        marker.pos, radius + 5.0,
        egui::Stroke::new(5.0, color.gamma_multiply(0.08)),
    );
    painter.circle_filled(marker.pos, radius, color);
    if is_selected {
        painter.circle_stroke(marker.pos, radius + 3.0, egui::Stroke::new(1.1, color));
    }
}

pub(super) fn marker_elevation_m(selected_root: Option<&Path>, point: GeoPoint) -> f32 {
    let terrain_elevation_m = srtm_stream::sample_elevation_m(selected_root, point).unwrap_or(0.0);
    terrain_elevation_m + 18.0
}
