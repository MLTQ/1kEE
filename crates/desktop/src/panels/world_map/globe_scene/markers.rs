use crate::arcgis_source;
use crate::model::{
    ActiveFlare, ArcGisFeature, EventRecord, FlightCategory, FlightTrack, GeoPoint, GlobeViewState,
    MovingTrack,
};
use crate::theme;

use super::projection::project_geo;
use super::{GlobeLayout, ProjectedPoint};

/// Draw all live AIS vessels as small ship markers on the globe.
pub(super) fn draw_ships(
    painter: &egui::Painter,
    layout: &GlobeLayout,
    view: &GlobeViewState,
    tracks: &[MovingTrack],
    selected_mmsi: Option<u64>,
) {
    // Cyan-teal — distinct from event (red/orange) and camera (blue) markers.
    let ship_color = egui::Color32::from_rgb(40, 210, 180);
    let selected_color = egui::Color32::from_rgb(255, 230, 80);

    for track in tracks {
        let Some(proj) = project_geo(layout, view, track.location, 0.0) else {
            continue;
        };
        if !proj.front_facing {
            continue;
        }

        let is_selected = selected_mmsi == Some(track.mmsi);
        let col = if is_selected {
            selected_color
        } else {
            ship_color
        };
        let pos = proj.pos;

        // Glow halo
        painter.circle_stroke(pos, 6.0, egui::Stroke::new(4.0, col.gamma_multiply(0.12)));

        // Heading arrow: draw a small directional triangle if heading is known.
        if let Some(heading) = track.heading_deg {
            let angle = heading.to_radians() - std::f32::consts::FRAC_PI_2;
            let fwd: f32 = 7.0;
            let back: f32 = 3.5;
            let wing: f32 = 3.0;

            // Tip of triangle (in heading direction)
            let tip = egui::pos2(pos.x + angle.cos() * fwd, pos.y + angle.sin() * fwd);
            // Left and right wing points (perpendicular to heading, slightly back)
            let angle_l = angle + std::f32::consts::FRAC_PI_2;
            let angle_r = angle - std::f32::consts::FRAC_PI_2;
            let left = egui::pos2(
                pos.x - angle.cos() * back + angle_l.cos() * wing,
                pos.y - angle.sin() * back + angle_l.sin() * wing,
            );
            let right = egui::pos2(
                pos.x - angle.cos() * back + angle_r.cos() * wing,
                pos.y - angle.sin() * back + angle_r.sin() * wing,
            );

            // Filled triangle (mesh)
            let mut mesh = egui::epaint::Mesh::default();
            let base_i = mesh.vertices.len() as u32;
            for &p in &[tip, left, right] {
                mesh.vertices.push(egui::epaint::Vertex {
                    pos: p,
                    uv: egui::pos2(0.0, 0.0),
                    color: col,
                });
            }
            mesh.indices
                .extend_from_slice(&[base_i, base_i + 1, base_i + 2]);
            painter.add(egui::Shape::mesh(mesh));
        } else {
            // No heading: draw a simple filled dot
            painter.circle_filled(pos, 3.0, col);
        }

        // Selection ring
        if is_selected {
            painter.circle_stroke(pos, 9.0, egui::Stroke::new(1.5, selected_color));
        }
    }
}

/// Draw all live ADS-B flights as small directional markers on the globe.
///
/// Colour scheme: callsign-derived category colours from the active theme,
/// so markers integrate with Topo / Phosphor / Thermal / Ghost / Akira palettes.
/// Vertical rate adds a subtle brightness modifier.  The selected flight gets
/// an outer selection ring so it stands out from the crowd.
pub(super) fn draw_flights(
    painter: &egui::Painter,
    layout: &GlobeLayout,
    view: &GlobeViewState,
    flights: &[FlightTrack],
    selected_icao24: Option<&str>,
) {
    for flight in flights {
        let Some(proj) = project_geo(layout, view, flight.location, 0.0) else {
            continue;
        };
        if !proj.front_facing {
            continue;
        }

        // ── Category → base colour (theme-aware) ───────────────────────────
        let cat_col: egui::Color32 = match flight.category() {
            FlightCategory::Airline => theme::flight_airline_color(),
            FlightCategory::Cargo => theme::flight_cargo_color(),
            FlightCategory::Military => theme::flight_military_color(),
            FlightCategory::GA => theme::flight_ga_color(),
            FlightCategory::Unknown => theme::flight_unknown_color(),
        };

        // ── Vertical-rate brightness modifier ──────────────────────────────
        let col = match flight.vertical_rate_fpm {
            Some(r) if r > 100.0 => cat_col.gamma_multiply(1.20),
            Some(r) if r < -100.0 => cat_col.gamma_multiply(0.80),
            _ => cat_col,
        };
        let pos = proj.pos;

        // Soft glow halo
        painter.circle_stroke(pos, 5.5, egui::Stroke::new(3.5, col.gamma_multiply(0.10)));

        if let Some(heading) = flight.heading_deg {
            // Small filled triangle pointing in the direction of travel.
            let angle = heading.to_radians() - std::f32::consts::FRAC_PI_2;
            let fwd: f32 = 6.0;
            let back: f32 = 3.0;
            let wing: f32 = 2.5;

            let tip = egui::pos2(pos.x + angle.cos() * fwd, pos.y + angle.sin() * fwd);
            let left = egui::pos2(
                pos.x - angle.cos() * back + (angle + std::f32::consts::FRAC_PI_2).cos() * wing,
                pos.y - angle.sin() * back + (angle + std::f32::consts::FRAC_PI_2).sin() * wing,
            );
            let right = egui::pos2(
                pos.x - angle.cos() * back + (angle - std::f32::consts::FRAC_PI_2).cos() * wing,
                pos.y - angle.sin() * back + (angle - std::f32::consts::FRAC_PI_2).sin() * wing,
            );

            let mut mesh = egui::epaint::Mesh::default();
            let base_i = mesh.vertices.len() as u32;
            for &p in &[tip, left, right] {
                mesh.vertices.push(egui::epaint::Vertex {
                    pos: p,
                    uv: egui::pos2(0.0, 0.0),
                    color: col,
                });
            }
            mesh.indices
                .extend_from_slice(&[base_i, base_i + 1, base_i + 2]);
            painter.add(egui::Shape::mesh(mesh));
        } else {
            painter.circle_filled(pos, 2.5, col);
        }

        // ── Selection ring ─────────────────────────────────────────────────
        if selected_icao24 == Some(flight.icao24.as_str()) {
            painter.circle_stroke(pos, 10.0, egui::Stroke::new(2.0, col));
            painter.circle_stroke(pos, 12.5, egui::Stroke::new(1.0, col.gamma_multiply(0.4)));
        }
    }
}

/// Draw a Factal event as a glowing surface-normal laser beam.
/// `base` is the ground-strike projected point; `tip` is the 3-D-projected
/// beam tip (not a screen-space offset, so perspective foreshortening is
/// correct).  The beam fades from opaque at the base to transparent at the
/// tip — as if light is emerging from the planet's surface.
pub(super) fn draw_event_marker(
    painter: &egui::Painter,
    base: ProjectedPoint,
    tip: egui::Pos2,
    event: &EventRecord,
    is_selected: bool,
    time: f64,
) {
    let col = event.severity.color();
    draw_beam(painter, base.pos, tip, col, 1.0);

    // ── Ground strike ────────────────────────────────────────────────────────
    if is_selected {
        let pulse = 9.0 + ((time as f32 * 2.6).sin() + 1.0) * 3.5;
        painter.circle_stroke(
            base.pos,
            pulse,
            egui::Stroke::new(1.3, theme::marker_glow_warm()),
        );
    }
    draw_ground_strike(painter, base.pos, col, 1.0);
}

/// Draw a replay flare: same beam geometry but alpha-faded, plus a one-shot
/// expanding spawn ring.
pub(super) fn draw_replay_flare(
    painter: &egui::Painter,
    base: ProjectedPoint,
    tip: egui::Pos2,
    flare: &ActiveFlare,
    wall_elapsed: f64,
) {
    let alpha = flare.alpha(wall_elapsed);
    if alpha <= 0.005 {
        return;
    }
    let col = flare.event.severity.color();
    draw_beam(painter, base.pos, tip, col, alpha);
    draw_ground_strike(painter, base.pos, col, alpha);

    // Expanding spawn ring — one-shot, fades and grows outward.
    let ring_a = flare.ring_alpha(wall_elapsed);
    if ring_a > 0.005 {
        let ring_r = flare.ring_radius(wall_elapsed);
        painter.circle_stroke(
            base.pos,
            ring_r,
            egui::Stroke::new(1.8 * ring_a, col.gamma_multiply(ring_a * 0.75)),
        );
        // Second inner ring for more pop on Critical.
        if matches!(flare.event.severity, crate::model::EventSeverity::Critical) {
            painter.circle_stroke(
                base.pos,
                ring_r * 0.6,
                egui::Stroke::new(1.2 * ring_a, col.gamma_multiply(ring_a * 0.5)),
            );
        }
    }
}

// ── Shared beam primitives ────────────────────────────────────────────────────

fn draw_beam(
    painter: &egui::Painter,
    base: egui::Pos2,
    tip: egui::Pos2,
    col: egui::Color32,
    alpha: f32,
) {
    let dx = tip.x - base.x;
    let dy = tip.y - base.y;

    // Atmospheric halos — taper in both width and alpha toward the tip.
    const HALO_SEGS: u32 = 7;
    for i in 0..HALO_SEGS {
        let t0 = i as f32 / HALO_SEGS as f32;
        let t1 = (i + 1) as f32 / HALO_SEGS as f32;
        let tm = (t0 + t1) * 0.5;
        let a = (1.0 - tm).powi(2) * alpha;
        let p0 = egui::pos2(base.x + dx * t0, base.y + dy * t0);
        let p1 = egui::pos2(base.x + dx * t1, base.y + dy * t1);
        painter.line_segment(
            [p0, p1],
            egui::Stroke::new((22.0 * a).max(0.5), col.gamma_multiply(0.04 * a)),
        );
        painter.line_segment(
            [p0, p1],
            egui::Stroke::new((11.0 * a).max(0.5), col.gamma_multiply(0.08 * a)),
        );
        painter.line_segment(
            [p0, p1],
            egui::Stroke::new((4.5 * a).max(0.5), col.gamma_multiply(0.16 * a)),
        );
    }

    // Tapering core — cubic alpha, narrows to a spike.
    const SEGS: u32 = 14;
    for i in 0..SEGS {
        let t0 = i as f32 / SEGS as f32;
        let t1 = (i + 1) as f32 / SEGS as f32;
        let tm = (t0 + t1) * 0.5;
        let falloff = 1.0 - tm;
        let a = falloff.powi(3) * alpha;
        let w_glow = (4.0 * falloff.powf(0.7)).max(0.4);
        let w_core = (1.7 * falloff.powf(0.7)).max(0.3);
        let p0 = egui::pos2(base.x + dx * t0, base.y + dy * t0);
        let p1 = egui::pos2(base.x + dx * t1, base.y + dy * t1);
        painter.line_segment(
            [p0, p1],
            egui::Stroke::new(w_glow, col.gamma_multiply(a * 0.30)),
        );
        painter.line_segment(
            [p0, p1],
            egui::Stroke::new(w_core, col.gamma_multiply(a * 0.96)),
        );
    }
}

fn draw_ground_strike(painter: &egui::Painter, pos: egui::Pos2, col: egui::Color32, alpha: f32) {
    painter.circle_stroke(
        pos,
        6.5,
        egui::Stroke::new(9.0, col.gamma_multiply(0.06 * alpha)),
    );
    painter.circle_stroke(
        pos,
        4.8,
        egui::Stroke::new(1.1, col.gamma_multiply(0.60 * alpha)),
    );
    painter.circle_filled(pos, 2.5, col.gamma_multiply(alpha));
}

pub(super) fn draw_camera_marker(
    painter: &egui::Painter,
    marker: ProjectedPoint,
    is_selected: bool,
) {
    let radius = 3.0 + marker.depth;
    let color = if is_selected {
        theme::marker_camera_ring()
    } else {
        theme::camera_color()
    };

    painter.circle_stroke(
        marker.pos,
        radius + 5.5,
        egui::Stroke::new(5.5, color.gamma_multiply(0.07)),
    );
    painter.circle_filled(marker.pos, radius, color);
    if is_selected {
        painter.circle_stroke(marker.pos, radius + 3.2, egui::Stroke::new(1.1, color));
    }
}

pub(super) fn draw_camera_links(
    painter: &egui::Painter,
    event_marker: egui::Pos2,
    camera_markers: &[(String, egui::Pos2)],
) {
    for (_, marker) in camera_markers {
        painter.line_segment(
            [event_marker, *marker],
            egui::Stroke::new(0.8, theme::camera_color().gamma_multiply(0.36)),
        );
    }
}
