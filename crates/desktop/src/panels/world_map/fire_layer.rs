//! Batched visual treatment for NASA FIRMS active-fire detections.
//!
//! Geographic projection stays in the globe/local scene that knows the active
//! camera; this module receives screen-space positions plus intensity and emits
//! one mesh. A global VIIRS day is tens of thousands of points, so they are
//! drawn as a single batched mesh rather than per-point shapes.

use crate::fire_source::{FireConfidence, FireDetection};

/// A detection already projected into the current scene.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ProjectedFire {
    pub(crate) pos: egui::Pos2,
    /// Fire radiative power in megawatts, straight from the source.
    frp_mw: f32,
    high_confidence: bool,
}

impl ProjectedFire {
    pub(crate) fn new(pos: egui::Pos2, detection: &FireDetection) -> Self {
        Self {
            pos,
            frp_mw: detection.frp_mw.max(0.0),
            high_confidence: detection.confidence == FireConfidence::High,
        }
    }
}

/// Clamp fire radiative power into the domain the curves below expect.
///
/// A negative or NaN input would send `ln` to NaN, and `f32::clamp` propagates
/// NaN rather than rejecting it — which would put NaN vertices in the mesh.
/// This keeps both helpers total whatever reaches them.
fn sanitize_frp(frp_mw: f32) -> f32 {
    if frp_mw.is_finite() {
        frp_mw.max(0.0)
    } else {
        0.0
    }
}

/// Map fire radiative power onto a marker radius in points.
///
/// FRP is extremely skewed — the median detection is ~5 MW and the daily
/// maximum is several hundred — so a logarithmic curve keeps ordinary
/// agricultural burning legible without letting one megafire dominate. The
/// floor keeps a 0 MW detection visible; the ceiling stops a single pixel from
/// swallowing its neighbours.
fn radius_for(frp_mw: f32) -> f32 {
    const MIN_RADIUS: f32 = 1.1;
    const MAX_RADIUS: f32 = 5.0;
    let scaled = MIN_RADIUS + (1.0 + sanitize_frp(frp_mw)).ln() * 0.62;
    scaled.clamp(MIN_RADIUS, MAX_RADIUS)
}

/// Heat ramp from deep ember to white-hot, keyed on the same log-FRP curve as
/// the radius so size and colour agree.
fn color_for(frp_mw: f32) -> egui::Color32 {
    // ~0 MW → 0.0, ~150 MW → 1.0.
    let t = ((1.0 + sanitize_frp(frp_mw)).ln() / 5.0).clamp(0.0, 1.0);
    let lerp = |a: f32, b: f32| (a + (b - a) * t) as u8;
    egui::Color32::from_rgb(lerp(224.0, 255.0), lerp(78.0, 232.0), lerp(28.0, 168.0))
}

/// Build every supplied detection into one mesh.
///
/// The orange/white heat palette is intentionally distinct from the violet ALPR
/// markers and the event severity colours.
pub(crate) fn build_mesh(fires: &[ProjectedFire]) -> std::sync::Arc<egui::epaint::Mesh> {
    if fires.is_empty() {
        return std::sync::Arc::new(egui::epaint::Mesh::default());
    }

    let mut mesh = egui::epaint::Mesh::default();
    for fire in fires {
        let radius = radius_for(fire.frp_mw);
        let color = color_for(fire.frp_mw);

        // A dim outer square reads as thermal bloom at a distance.
        push_square(
            &mut mesh,
            fire.pos,
            radius * 1.9,
            egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 34),
        );
        push_square(&mut mesh, fire.pos, radius, color);
        // High-confidence detections get a hot core so they stand out from the
        // nominal-confidence field without needing a separate legend.
        if fire.high_confidence {
            push_square(
                &mut mesh,
                fire.pos,
                (radius * 0.42).max(0.55),
                egui::Color32::from_rgb(255, 248, 226),
            );
        }
    }

    std::sync::Arc::new(mesh)
}

/// Submit a previously-built fire mesh. Both scenes cache this while the data
/// snapshot and camera transform are unchanged.
pub(crate) fn draw_mesh(painter: &egui::Painter, mesh: std::sync::Arc<egui::epaint::Mesh>) {
    if !mesh.vertices.is_empty() {
        painter.add(egui::Shape::mesh(mesh));
    }
}

/// Push an axis-aligned quad. Squares rather than discs keep the vertex count
/// at four per mark, which matters at tens of thousands of detections.
fn push_square(mesh: &mut egui::epaint::Mesh, center: egui::Pos2, half: f32, color: egui::Color32) {
    let base = mesh.vertices.len() as u32;
    let rect = egui::Rect::from_center_size(center, egui::vec2(half * 2.0, half * 2.0));
    for pos in [
        rect.left_top(),
        rect.right_top(),
        rect.right_bottom(),
        rect.left_bottom(),
    ] {
        mesh.vertices.push(egui::epaint::Vertex {
            pos,
            uv: egui::epaint::WHITE_UV,
            color,
        });
    }
    mesh.indices
        .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn radius_and_colour_rise_with_intensity_then_saturate() {
        assert!(radius_for(0.0) < radius_for(10.0));
        assert!(radius_for(10.0) < radius_for(100.0));
        // A megafire cannot grow without bound.
        assert_eq!(radius_for(1_000.0), radius_for(10_000.0));

        assert!(color_for(200.0).g() > color_for(1.0).g());
    }

    #[test]
    fn handles_a_degenerate_detection() {
        // FRP is absent in some rows and parses to 0.0; it must still draw.
        assert!(radius_for(0.0) > 0.0);
        // Negative and NaN inputs must not reach `ln`: a NaN radius would put
        // NaN vertices into the mesh, and `f32::clamp` propagates NaN.
        assert!(radius_for(-5.0).is_finite());
        assert!(radius_for(f32::NAN).is_finite());
        assert_eq!(radius_for(-5.0), radius_for(0.0));
        assert!(color_for(f32::NAN) == color_for(0.0));
    }

    #[test]
    fn builds_four_vertices_per_mark() {
        let detection = FireDetection {
            location: crate::model::GeoPoint { lat: 0.0, lon: 0.0 },
            frp_mw: 12.0,
            confidence: FireConfidence::Nominal,
            acquired_date: "2026-09-21".into(),
            acquired_time: "0100".into(),
            satellite: "test",
        };
        let fire = ProjectedFire::new(egui::pos2(10.0, 10.0), &detection);
        let mesh = build_mesh(&[fire]);
        // Bloom + body for a nominal detection.
        assert_eq!(mesh.vertices.len(), 8);
        assert_eq!(mesh.indices.len(), 12);
    }

    #[test]
    fn high_confidence_adds_a_core() {
        let detection = FireDetection {
            location: crate::model::GeoPoint { lat: 0.0, lon: 0.0 },
            frp_mw: 12.0,
            confidence: FireConfidence::High,
            acquired_date: String::new(),
            acquired_time: String::new(),
            satellite: "test",
        };
        let fire = ProjectedFire::new(egui::pos2(0.0, 0.0), &detection);
        assert_eq!(build_mesh(&[fire]).vertices.len(), 12);
    }

    #[test]
    fn empty_input_produces_an_empty_mesh() {
        assert!(build_mesh(&[]).vertices.is_empty());
    }
}
