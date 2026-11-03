//! Drape a native gravity window on the local sea-level plane.
use super::super::local_terrain_scene::LocalLayout;
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
    let Some(image) = super::texture(painter.ctx(), root, Some(region), super::Scene::Local) else {
        return;
    };
    let Ok(image) = image.as_ref() else {
        return;
    };
    let Some(region) = image.region else {
        return;
    };
    let mesh = super::super::raster_mesh::local(
        layout,
        view,
        image.texture.id(),
        region.bounds(),
        focus,
        half,
        painter.clip_rect(),
    );
    painter.add(egui::Shape::mesh(mesh));
}
