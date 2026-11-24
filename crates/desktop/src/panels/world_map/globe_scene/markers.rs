use super::super::marker_style::{MapMarker, draw_beam};
use crate::model::{ActiveFlare, EventRecord, FlightCategory, FlightTrack, MovingTrack};
use crate::theme;

use super::{ProjectedMarker, ProjectedPoint};

const RING_SIDES: usize = 10;

/// Precomputed unit-circle offsets for fast batched marker rings/discs.
fn unit_ring() -> [(f32, f32); RING_SIDES] {
    let mut out = [(0.0_f32, 0.0_f32); RING_SIDES];
    for (i, slot) in out.iter_mut().enumerate() {
        let a = (i as f32) * (std::f32::consts::TAU / RING_SIDES as f32);
        *slot = (a.cos(), a.sin());
    }
    out
}

#[inline]
fn push_ring(
    mesh: &mut egui::epaint::Mesh,
    ring: &[(f32, f32); RING_SIDES],
    pos: egui::Pos2,
    inner_r: f32,
    outer_r: f32,
    col: egui::Color32,
) {
    let start = mesh.vertices.len() as u32;
    for &(cx, cy) in ring {
        mesh.colored_vertex(egui::pos2(pos.x + cx * inner_r, pos.y + cy * inner_r), col);
        mesh.colored_vertex(egui::pos2(pos.x + cx * outer_r, pos.y + cy * outer_r), col);
    }
    for i in 0..RING_SIDES as u32 {
        let next = (i + 1) % RING_SIDES as u32;
        let i0 = start + i * 2;
        let i1 = start + next * 2;
        mesh.add_triangle(i0, i0 + 1, i1);
        mesh.add_triangle(i0 + 1, i1 + 1, i1);
    }
}

#[inline]
fn push_disc(
    mesh: &mut egui::epaint::Mesh,
    ring: &[(f32, f32); RING_SIDES],
    pos: egui::Pos2,
    r: f32,
    col: egui::Color32,
) {
    let center = mesh.vertices.len() as u32;
    mesh.colored_vertex(pos, col);
    for &(cx, cy) in ring {
        mesh.colored_vertex(egui::pos2(pos.x + cx * r, pos.y + cy * r), col);
    }
    for i in 0..RING_SIDES as u32 {
        let next = (i + 1) % RING_SIDES as u32;
        mesh.add_triangle(center, center + 1 + i, center + 1 + next);
    }
}

#[inline]
fn push_heading_tri(
    mesh: &mut egui::epaint::Mesh,
    pos: egui::Pos2,
    heading_deg: f32,
    fwd: f32,
    back: f32,
    wing: f32,
    col: egui::Color32,
) {
    let angle = heading_deg.to_radians() - std::f32::consts::FRAC_PI_2;
    let (sin_a, cos_a) = angle.sin_cos();
    // Perpendicular vector (angle + pi/2) is (-sin_a, cos_a); zero extra trig calls.
    let tip = egui::pos2(pos.x + cos_a * fwd, pos.y + sin_a * fwd);
    let bx = pos.x - cos_a * back;
    let by = pos.y - sin_a * back;
    let left = egui::pos2(bx - sin_a * wing, by + cos_a * wing);
    let right = egui::pos2(bx + sin_a * wing, by - cos_a * wing);
    let base_i = mesh.vertices.len() as u32;
    mesh.colored_vertex(tip, col);
    mesh.colored_vertex(left, col);
    mesh.colored_vertex(right, col);
    mesh.add_triangle(base_i, base_i + 1, base_i + 2);
}

/// Draw all live AIS vessels as small ship markers on the globe in a single batched mesh.
pub(super) fn draw_ships(
    painter: &egui::Painter,
    tracks: &[MovingTrack],
    projected_tracks: &[ProjectedMarker],
    selected_mmsi: Option<u64>,
) {
    if projected_tracks.is_empty() {
        return;
    }
    let ship_color = egui::Color32::from_rgb(40, 210, 180);
    let selected_color = egui::Color32::from_rgb(255, 230, 80);
    let ship_halo = ship_color.gamma_multiply(0.12);
    let selected_halo = selected_color.gamma_multiply(0.12);
    let ring = unit_ring();

    let mut mesh = egui::epaint::Mesh::default();
    mesh.vertices.reserve(projected_tracks.len() * (RING_SIDES * 2 + 4));
    mesh.indices.reserve(projected_tracks.len() * (RING_SIDES * 6 + 3));
    let mut selected_pos = None;

    for projected in projected_tracks {
        let track = &tracks[projected.source_index];
        let is_selected = selected_mmsi == Some(track.mmsi);
        let (col, halo) = if is_selected {
            (selected_color, selected_halo)
        } else {
            (ship_color, ship_halo)
        };
        let pos = projected.point.pos;

        // Soft glow halo (center radius 6.0, stroke width 4.0 -> [4.0, 8.0])
        push_ring(&mut mesh, &ring, pos, 4.0, 8.0, halo);

        if let Some(heading) = track.heading_deg {
            push_heading_tri(&mut mesh, pos, heading, 7.0, 3.5, 3.0, col);
        } else {
            push_disc(&mut mesh, &ring, pos, 3.0, col);
        }

        if is_selected {
            selected_pos = Some(pos);
        }
    }

    if !mesh.indices.is_empty() {
        painter.add(egui::Shape::mesh(mesh));
    }
    if let Some(pos) = selected_pos {
        painter.circle_stroke(pos, 9.0, egui::Stroke::new(1.5, selected_color));
    }
}

/// Draw all live ADS-B flights as small directional markers on the globe in a single batched mesh.
pub(super) fn draw_flights(
    painter: &egui::Painter,
    flights: &[FlightTrack],
    projected_flights: &[ProjectedMarker],
    selected_icao24: Option<&str>,
) {
    if projected_flights.is_empty() {
        return;
    }
    // Hoist all 5 theme palette lookups outside the per-flight loop.
    let c_airline = theme::flight_airline_color();
    let c_cargo = theme::flight_cargo_color();
    let c_military = theme::flight_military_color();
    let c_ga = theme::flight_ga_color();
    let c_unknown = theme::flight_unknown_color();
    let ring = unit_ring();

    let mut mesh = egui::epaint::Mesh::default();
    mesh.vertices.reserve(projected_flights.len() * (RING_SIDES * 2 + 4));
    mesh.indices.reserve(projected_flights.len() * (RING_SIDES * 6 + 3));
    let mut selected_ring: Option<(egui::Pos2, egui::Color32)> = None;

    for projected in projected_flights {
        let flight = &flights[projected.source_index];
        let cat_col = match flight.category() {
            FlightCategory::Airline => c_airline,
            FlightCategory::Cargo => c_cargo,
            FlightCategory::Military => c_military,
            FlightCategory::GA => c_ga,
            FlightCategory::Unknown => c_unknown,
        };

        let col = match flight.vertical_rate_fpm {
            Some(r) if r > 100.0 => cat_col.gamma_multiply(1.20),
            Some(r) if r < -100.0 => cat_col.gamma_multiply(0.80),
            _ => cat_col,
        };
        let pos = projected.point.pos;

        // Soft glow halo (center radius 5.5, width 3.5 -> [3.75, 7.25])
        push_ring(&mut mesh, &ring, pos, 3.75, 7.25, col.gamma_multiply(0.10));

        if let Some(heading) = flight.heading_deg {
            push_heading_tri(&mut mesh, pos, heading, 6.0, 3.0, 2.5, col);
        } else {
            push_disc(&mut mesh, &ring, pos, 2.5, col);
        }

        if selected_icao24 == Some(flight.icao24.as_str()) {
            selected_ring = Some((pos, col));
        }
    }

    if !mesh.indices.is_empty() {
        painter.add(egui::Shape::mesh(mesh));
    }
    if let Some((pos, col)) = selected_ring {
        painter.circle_stroke(pos, 10.0, egui::Stroke::new(2.0, col));
        painter.circle_stroke(pos, 12.5, egui::Stroke::new(1.0, col.gamma_multiply(0.4)));
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
    scale: f32,
) {
    let col = event.severity.color();
    draw_beam(painter, base.pos, tip, col, 1.0, scale);

    // ── Ground strike ────────────────────────────────────────────────────────
    if is_selected {
        let pulse = 9.0 + ((time as f32 * 2.6).sin() + 1.0) * 3.5;
        painter.circle_stroke(
            base.pos,
            pulse * scale,
            egui::Stroke::new(1.3 * scale, theme::marker_glow_warm()),
        );
    }
    draw_ground_strike(painter, base.pos, col, 1.0, scale);
}

/// Draw a replay flare: same beam geometry but alpha-faded, plus a one-shot
/// expanding spawn ring.
pub(super) fn draw_replay_flare(
    painter: &egui::Painter,
    base: ProjectedPoint,
    tip: egui::Pos2,
    flare: &ActiveFlare,
    wall_elapsed: f64,
    scale: f32,
) {
    let alpha = flare.alpha(wall_elapsed);
    if alpha <= 0.005 {
        return;
    }
    let col = flare.event.severity.color();
    draw_beam(painter, base.pos, tip, col, alpha, scale);
    draw_ground_strike(painter, base.pos, col, alpha, scale);

    // Expanding spawn ring — one-shot, fades and grows outward.
    let ring_a = flare.ring_alpha(wall_elapsed);
    if ring_a > 0.005 {
        let ring_r = flare.ring_radius(wall_elapsed) * scale;
        painter.circle_stroke(
            base.pos,
            ring_r,
            egui::Stroke::new(1.8 * ring_a * scale, col.gamma_multiply(ring_a * 0.75)),
        );
        // Second inner ring for more pop on Critical.
        if matches!(flare.event.severity, crate::model::EventSeverity::Critical) {
            painter.circle_stroke(
                base.pos,
                ring_r * 0.6,
                egui::Stroke::new(1.2 * ring_a * scale, col.gamma_multiply(ring_a * 0.5)),
            );
        }
    }
}

// ── Shared beam primitives ────────────────────────────────────────────────────

fn draw_ground_strike(
    painter: &egui::Painter,
    pos: egui::Pos2,
    col: egui::Color32,
    alpha: f32,
    scale: f32,
) {
    painter.circle_stroke(
        pos,
        6.5 * scale,
        egui::Stroke::new(9.0 * scale, col.gamma_multiply(0.06 * alpha)),
    );
    painter.circle_stroke(
        pos,
        4.8 * scale,
        egui::Stroke::new(1.1 * scale, col.gamma_multiply(0.60 * alpha)),
    );
    painter.circle_filled(pos, 2.5 * scale, col.gamma_multiply(alpha));
}

pub(super) fn draw_camera_links(
    painter: &egui::Painter,
    event_marker: egui::Pos2,
    camera_markers: &[MapMarker],
) {
    for marker in camera_markers {
        painter.line_segment(
            [event_marker, marker.base],
            egui::Stroke::new(0.8, theme::camera_color().gamma_multiply(0.36)),
        );
    }
}
