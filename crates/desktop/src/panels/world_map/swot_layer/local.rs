//! Drape a native gravity window on the local sea-level plane.
use super::super::local_terrain_scene::{LocalLayout, project_local};
use crate::model::{GeoPoint, GlobeViewState};
use std::path::Path;

pub(in super::super) fn draw(
    painter: &egui::Painter,
    layout: &LocalLayout,
    view: &GlobeViewState,
    root: Option<&Path>,
    focus: GeoPoint,
    half: f32,
) {
    let region = super::grid::Region::around(focus.lat, focus.lon, half);
    let Some(image) = super::texture(painter.ctx(), root, Some(region)) else {
        return;
    };
    let Ok(texture) = image.as_ref() else {
        return;
    };
    let [west, south, east, north] = region.bounds();
    let extent_y = (half * 111.32).max(1.0);
    let extent_x = (half * 111.32 * focus.lat.to_radians().cos().abs().max(0.2)).max(1.0);
    let mut mesh = egui::Mesh::with_texture(texture.id());
    const CELLS: usize = 48;
    let mut indices = Vec::with_capacity((CELLS + 1).pow(2));
    for row in 0..=CELLS {
        let v = row as f32 / CELLS as f32;
        for col in 0..=CELLS {
            let u = col as f32 / CELLS as f32;
            let point = GeoPoint {
                lat: north + (south - north) * v,
                lon: west + (east - west) * u,
            };
            let index =
                project_local(layout, view, focus, point, 0.0, extent_x, extent_y).map(|p| {
                    let index = mesh.vertices.len() as u32;
                    mesh.vertices.push(egui::epaint::Vertex {
                        pos: p.pos,
                        uv: egui::pos2(u, v),
                        color: egui::Color32::WHITE,
                    });
                    index
                });
            indices.push(index);
        }
    }
    for row in 0..CELLS {
        for col in 0..CELLS {
            let a = row * (CELLS + 1) + col;
            if let [Some(a), Some(b), Some(c), Some(d)] = [
                indices[a],
                indices[a + 1],
                indices[a + CELLS + 2],
                indices[a + CELLS + 1],
            ] {
                mesh.indices.extend_from_slice(&[a, b, c, a, c, d]);
            }
        }
    }
    painter.add(egui::Shape::mesh(mesh));
}
