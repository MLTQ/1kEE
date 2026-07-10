//! Batched visual treatment for public DeFlock ALPR locations.
//!
//! Geographic projection stays in the globe/local scene that knows the active
//! camera. This module receives only screen-space positions and optional public
//! direction metadata, keeping the import path and rendering lifetime separate.

use crate::model::GeoPoint;

/// A public ALPR point that has already been projected into the current scene.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ProjectedAlprMarker {
    pub(crate) pos: egui::Pos2,
    direction_radians: Option<f32>,
}

impl ProjectedAlprMarker {
    /// Builds a marker from a screen position and a direction already
    /// transformed through the active map projection. Non-finite directions
    /// are deliberately treated as unknown rather than producing invalid mesh
    /// vertices.
    pub(crate) fn new(pos: egui::Pos2, direction_radians: Option<f32>) -> Self {
        Self {
            pos,
            direction_radians: direction_radians.filter(|direction| direction.is_finite()),
        }
    }
}

/// Returns a nearby geographic point along a public clockwise-from-north ALPR
/// bearing. Scene code projects both ends to derive the exact screen direction
/// under globe rotation, perspective, or local yaw.
pub(crate) fn bearing_target(
    location: GeoPoint,
    direction_degrees: Option<f32>,
    step_degrees: f32,
) -> Option<GeoPoint> {
    let bearing = direction_degrees?.to_radians();
    let distance = step_degrees.to_radians();
    if !bearing.is_finite() || !distance.is_finite() || distance <= 0.0 {
        return None;
    }
    let lat1 = location.lat.to_radians();
    let lon1 = location.lon.to_radians();
    let lat2 = (lat1.sin() * distance.cos() + lat1.cos() * distance.sin() * bearing.cos()).asin();
    let lon2 = lon1
        + (bearing.sin() * distance.sin() * lat1.cos())
            .atan2(distance.cos() - lat1.sin() * lat2.sin());
    let lat = lat2.to_degrees();
    let lon = (lon2.to_degrees() + 180.0).rem_euclid(360.0) - 180.0;
    (lat.is_finite() && lon.is_finite()).then_some(GeoPoint { lat, lon })
}

/// Build every supplied public ALPR location into one mesh.
///
/// The violet diamond-and-wedge icon is intentionally distinct from the
/// application camera, event, vessel, flight, and ArcGIS marker palettes. A
/// direction wedge is shown only when the public source provides one.
pub(crate) fn build_mesh(markers: &[ProjectedAlprMarker]) -> std::sync::Arc<egui::epaint::Mesh> {
    if markers.is_empty() {
        return std::sync::Arc::new(egui::epaint::Mesh::default());
    }

    let mut mesh = egui::epaint::Mesh::default();
    let halo = egui::Color32::from_rgba_unmultiplied(218, 84, 255, 38);
    let body = egui::Color32::from_rgba_unmultiplied(226, 95, 255, 218);
    let core = egui::Color32::from_rgb(255, 225, 255);

    for marker in markers {
        // A small outer diamond gives the point a soft sensor-footprint halo.
        push_diamond(&mut mesh, marker.pos, 5.8, halo);
        if let Some(angle) = marker.direction_radians {
            push_direction_wedge(&mut mesh, marker.pos, angle, body);
        }
        push_diamond(&mut mesh, marker.pos, 2.35, core);
    }

    std::sync::Arc::new(mesh)
}

/// Submit a previously-built ALPR mesh. Globe mode caches this mesh while the
/// data snapshot and camera transform are unchanged, avoiding repeat work for
/// large public point sets without altering the marker pixels.
pub(crate) fn draw_mesh(painter: &egui::Painter, mesh: std::sync::Arc<egui::epaint::Mesh>) {
    if !mesh.vertices.is_empty() {
        painter.add(egui::Shape::mesh(mesh));
    }
}

fn push_diamond(
    mesh: &mut egui::epaint::Mesh,
    center: egui::Pos2,
    radius: f32,
    color: egui::Color32,
) {
    let base_index = mesh.vertices.len() as u32;
    let positions = [
        egui::pos2(center.x, center.y - radius),
        egui::pos2(center.x + radius, center.y),
        egui::pos2(center.x, center.y + radius),
        egui::pos2(center.x - radius, center.y),
    ];
    for pos in positions {
        mesh.vertices.push(egui::epaint::Vertex {
            pos,
            uv: egui::pos2(0.0, 0.0),
            color,
        });
    }
    mesh.indices.extend_from_slice(&[
        base_index,
        base_index + 1,
        base_index + 2,
        base_index,
        base_index + 2,
        base_index + 3,
    ]);
}

fn push_direction_wedge(
    mesh: &mut egui::epaint::Mesh,
    center: egui::Pos2,
    angle: f32,
    color: egui::Color32,
) {
    let forward_x = angle.cos();
    let forward_y = angle.sin();
    let side_x = -forward_y;
    let side_y = forward_x;
    let base_index = mesh.vertices.len() as u32;
    let positions = [
        egui::pos2(center.x + forward_x * 8.0, center.y + forward_y * 8.0),
        egui::pos2(
            center.x + forward_x * 1.25 + side_x * 2.5,
            center.y + forward_y * 1.25 + side_y * 2.5,
        ),
        egui::pos2(
            center.x + forward_x * 1.25 - side_x * 2.5,
            center.y + forward_y * 1.25 - side_y * 2.5,
        ),
    ];
    for pos in positions {
        mesh.vertices.push(egui::epaint::Vertex {
            pos,
            uv: egui::pos2(0.0, 0.0),
            color,
        });
    }
    mesh.indices
        .extend_from_slice(&[base_index, base_index + 1, base_index + 2]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projected_direction_is_retained_and_invalid_values_are_ignored() {
        let east = ProjectedAlprMarker::new(egui::Pos2::ZERO, Some(0.0));
        assert_eq!(east.direction_radians, Some(0.0));

        let unknown = ProjectedAlprMarker::new(egui::Pos2::ZERO, Some(f32::NAN));
        assert_eq!(unknown.direction_radians, None);
    }

    #[test]
    fn bearing_target_moves_north_without_changing_longitude_at_the_equator() {
        let target = bearing_target(
            GeoPoint {
                lat: 0.0,
                lon: 10.0,
            },
            Some(0.0),
            0.1,
        )
        .unwrap();
        assert!(target.lat > 0.0);
        assert!((target.lon - 10.0).abs() < 0.0001);
    }
}
