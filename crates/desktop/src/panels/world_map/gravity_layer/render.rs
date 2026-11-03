//! Project the selected regional gravity image in Globe and Local views.
use super::super::{globe_scene::GlobeLayout, local_terrain_scene::LocalLayout};
use super::texture;
use crate::model::{GeoPoint, GlobeViewState};
use std::cell::RefCell;
use std::path::Path;
use std::sync::Arc;

type MeshCache = Option<(egui::TextureId, [u32; 7], Arc<egui::Mesh>)>;
thread_local! { static MESH: RefCell<MeshCache> = const { RefCell::new(None) }; }

pub(super) fn clear() {
    MESH.with(|slot| *slot.borrow_mut() = None);
}

pub(in super::super) fn draw_globe(
    painter: &egui::Painter,
    layout: &GlobeLayout,
    view: &GlobeViewState,
    root: Option<&Path>,
    id: &str,
) {
    let Some(image) = texture(painter.ctx(), root, id) else {
        return;
    };
    let Ok(image) = image.as_ref() else {
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
            && *id == image.texture.id()
            && *old == key
        {
            return mesh.clone();
        }
        let mesh = Arc::new(super::super::raster_mesh::globe(
            layout,
            view,
            image.texture.id(),
            image.bounds,
            true,
        ));
        *slot = Some((image.texture.id(), key, mesh.clone()));
        mesh
    });
    painter.add(egui::Shape::mesh(mesh));
}

pub(in super::super) fn draw_local(
    painter: &egui::Painter,
    layout: &LocalLayout,
    view: &GlobeViewState,
    root: Option<&Path>,
    id: &str,
    focus: GeoPoint,
    half: f32,
) {
    let Some(image) = texture(painter.ctx(), root, id) else {
        return;
    };
    let Ok(image) = image.as_ref() else {
        return;
    };
    painter.add(egui::Shape::mesh(super::super::raster_mesh::local(
        layout,
        view,
        image.texture.id(),
        image.bounds,
        focus,
        half,
        painter.clip_rect(),
    )));
}
