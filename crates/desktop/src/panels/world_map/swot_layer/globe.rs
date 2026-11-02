//! Project the overview onto the visible Earth sphere, cached across still frames.
use super::super::globe_scene::{GlobeLayout, project_surface};
use crate::model::{GeoPoint, GlobeViewState};
use std::cell::RefCell;
use std::path::Path;
use std::sync::Arc;

type MeshCache = Option<(egui::TextureId, [u32; 7], Arc<egui::Mesh>)>;
thread_local! { static MESH: RefCell<MeshCache> = const { RefCell::new(None) }; }

pub(super) fn clear() {
    MESH.with(|slot| *slot.borrow_mut() = None);
}

pub(in super::super) fn draw(
    painter: &egui::Painter,
    layout: &GlobeLayout,
    view: &GlobeViewState,
    root: Option<&Path>,
) {
    let Some(image) = super::texture(painter.ctx(), root, None) else {
        return;
    };
    let Ok(texture) = image.as_ref() else {
        return;
    };
    let key = [
        view.yaw.to_bits(),
        view.pitch.to_bits(),
        layout.radius.to_bits(),
        layout.focal_length.to_bits(),
        layout.camera_distance.to_bits(),
        layout.center.x.to_bits(),
        layout.center.y.to_bits(),
    ];
    let mesh = MESH.with(|slot| {
        let mut slot = slot.borrow_mut();
        if let Some((id, old, mesh)) = slot.as_ref()
            && *id == texture.id()
            && *old == key
        {
            return mesh.clone();
        }
        let mesh = Arc::new(build(layout, view, texture.id()));
        *slot = Some((texture.id(), key, mesh.clone()));
        mesh
    });
    painter.add(egui::Shape::mesh(mesh));
}

fn build(layout: &GlobeLayout, view: &GlobeViewState, texture: egui::TextureId) -> egui::Mesh {
    let mut mesh = egui::Mesh::with_texture(texture);
    // Exact global edge coverage, separate UVs on each side of the dateline.
    for row in 0..90 {
        let south = -90.0 + row as f32 * 2.0;
        for col in 0..180 {
            let west = -180.0 + col as f32 * 2.0;
            let corners = [
                (south + 2.0, west),
                (south + 2.0, west + 2.0),
                (south, west + 2.0),
                (south, west),
            ];
            let projected =
                corners.map(|(lat, lon)| project_surface(layout, view, GeoPoint { lat, lon }));
            // A perspective camera sees only z >= 1/d on the unit sphere;
            // hemisphere culling alone would paint hidden surface near the limb.
            if !projected.iter().all(|p| {
                p.is_some_and(|p| {
                    p.front_facing && p.depth * 2.0 - 1.0 >= 1.0 / layout.camera_distance
                })
            }) {
                continue;
            }
            let index = mesh.vertices.len() as u32;
            for (i, (lat, lon)) in corners.into_iter().enumerate() {
                mesh.vertices.push(egui::epaint::Vertex {
                    pos: projected[i].unwrap().pos,
                    uv: egui::pos2((lon + 180.0) / 360.0, (90.0 - lat) / 180.0),
                    color: egui::Color32::WHITE,
                });
            }
            mesh.indices.extend_from_slice(&[
                index,
                index + 1,
                index + 2,
                index,
                index + 2,
                index + 3,
            ]);
        }
    }
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mesh_stays_inside_perspective_horizon_and_global_uv_bounds() {
        let view = GlobeViewState::from_focus(GeoPoint { lat: 0.0, lon: 0.0 });
        let layout = GlobeLayout {
            center: egui::Pos2::ZERO,
            radius: 300.0,
            focal_length: 2.0,
            camera_distance: 3.0,
        };
        let mesh = build(&layout, &view, egui::TextureId::Managed(1));
        assert!(!mesh.vertices.is_empty());
        let limit =
            layout.radius * layout.focal_length / (layout.camera_distance.powi(2) - 1.0).sqrt();
        for vertex in mesh.vertices {
            assert!(vertex.pos.distance(layout.center) <= limit + 0.001);
            assert!((0.0..=1.0).contains(&vertex.uv.x));
            assert!((0.0..=1.0).contains(&vertex.uv.y));
        }
    }
}
