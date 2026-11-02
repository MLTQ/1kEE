//! Keep the global overview visible while a zoom-dependent native window loads.
use super::super::globe_scene::{GlobeLayout, project_surface};
use crate::model::{GeoPoint, GlobeViewState};
use std::cell::RefCell;
use std::path::Path;
use std::sync::Arc;

type MeshCache = Option<(egui::TextureId, [u32; 7], Arc<egui::Mesh>)>;
thread_local! { static MESH: RefCell<[MeshCache; 2]> = const { RefCell::new([None, None]) }; }

pub(super) fn clear() {
    MESH.with(|slot| *slot.borrow_mut() = [None, None]);
}

pub(in super::super) fn draw(
    painter: &egui::Painter,
    rect: egui::Rect,
    layout: &GlobeLayout,
    view: &GlobeViewState,
    root: Option<&Path>,
) {
    if let Some(image) = super::texture(painter.ctx(), root, None, super::Scene::Overview)
        && let Ok(image) = image.as_ref()
    {
        paint_image(painter, layout, view, image, 0);
    }
    if let Some(region) =
        super::viewport::detail_region(rect, layout, view, painter.ctx().pixels_per_point())
        && let Some(image) =
            super::texture(painter.ctx(), root, Some(region), super::Scene::GlobeDetail)
        && let Ok(image) = image.as_ref()
    {
        paint_image(painter, layout, view, image, 1);
    }
}

fn paint_image(
    painter: &egui::Painter,
    layout: &GlobeLayout,
    view: &GlobeViewState,
    image: &super::LoadedImage,
    index: usize,
) {
    let texture = &image.texture;
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
        let mut slots = slot.borrow_mut();
        let slot = &mut slots[index];
        if let Some((id, old, mesh)) = slot.as_ref()
            && *id == texture.id()
            && *old == key
        {
            return mesh.clone();
        }
        let mesh = Arc::new(build(layout, view, texture.id(), image.region));
        *slot = Some((texture.id(), key, mesh.clone()));
        mesh
    });
    painter.add(egui::Shape::mesh(mesh));
}

fn build(
    layout: &GlobeLayout,
    view: &GlobeViewState,
    texture: egui::TextureId,
    region: Option<super::grid::Region>,
) -> egui::Mesh {
    let mut mesh = egui::Mesh::with_texture(texture);
    let [left, bottom, right, top] = region.map_or([-180.0, -90.0, 180.0, 90.0], |r| r.bounds());
    let scale = layout.radius * layout.focal_length / (layout.camera_distance - 1.0);
    let step = if region.is_some() {
        (0.5 / scale).sqrt().to_degrees().clamp(0.25, 2.0)
    } else {
        2.0
    };
    let rows = ((top - bottom) / step).ceil() as usize;
    let cols = ((right - left) / step).ceil() as usize;
    // Both textures use their own exact geographic edges and UV map.
    for row in 0..rows {
        let south = bottom + (top - bottom) * row as f32 / rows as f32;
        let north = bottom + (top - bottom) * (row + 1) as f32 / rows as f32;
        for col in 0..cols {
            let west = left + (right - left) * col as f32 / cols as f32;
            let east = left + (right - left) * (col + 1) as f32 / cols as f32;
            let corners = [(north, west), (north, east), (south, east), (south, west)];
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
                    uv: egui::pos2((lon - left) / (right - left), (top - lat) / (top - bottom)),
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
        let mesh = build(&layout, &view, egui::TextureId::Managed(1), None);
        assert!(!mesh.vertices.is_empty());
        let limit =
            layout.radius * layout.focal_length / (layout.camera_distance.powi(2) - 1.0).sqrt();
        for vertex in mesh.vertices {
            assert!(vertex.pos.distance(layout.center) <= limit + 0.001);
            assert!((0.0..=1.0).contains(&vertex.uv.x));
            assert!((0.0..=1.0).contains(&vertex.uv.y));
        }
    }

    #[test]
    fn native_date_line_window_uses_its_own_uv_edges() {
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1536.0, 864.0));
        let mut view = GlobeViewState::from_focus(GeoPoint {
            lat: 0.0,
            lon: 179.9,
        });
        view.zoom = 50.0;
        let layout = super::super::super::globe_scene::globe_layout(rect, &view);
        let region = super::super::grid::Region::from_bounds(170.0, -10.0, 190.0, 10.0);
        let mesh = build(&layout, &view, egui::TextureId::Managed(2), Some(region));
        assert!(!mesh.vertices.is_empty());
        assert!(mesh.vertices.iter().any(|v| v.uv == egui::pos2(0.0, 0.0)));
        assert!(mesh.vertices.iter().any(|v| v.uv == egui::pos2(1.0, 1.0)));
        for vertex in mesh.vertices {
            let point = GeoPoint {
                lat: 10.0 - vertex.uv.y * 20.0,
                lon: 170.0 + vertex.uv.x * 20.0,
            };
            let expected = project_surface(&layout, &view, point).unwrap();
            assert!(vertex.pos.distance(expected.pos) < 0.01);
        }
    }
}
