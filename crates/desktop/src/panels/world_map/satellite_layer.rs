//! User-imported weather imagery in the desktop Earth views.
use super::{globe_scene::GlobeLayout, local_terrain_scene::LocalLayout, raster_mesh};
use crate::{
    model::{ActiveBody, AppModel, GeoPoint, GlobeViewState},
    satellite_image::SatelliteImage,
};
use std::{cell::RefCell, sync::Arc};

type GlobeMesh = ([u32; 8], Arc<egui::Mesh>);

pub(crate) struct SatelliteLayer {
    name: String,
    visible: bool,
    opacity: u8,
    bounds: [f32; 4],
    texture: egui::TextureHandle,
    globe_mesh: RefCell<Option<GlobeMesh>>,
}

impl SatelliteLayer {
    pub(crate) fn new(ctx: &egui::Context, name: String, image: SatelliteImage) -> Self {
        let texture = ctx.load_texture(&name, image.pixels, egui::TextureOptions::LINEAR);
        Self {
            name,
            texture,
            bounds: image.bounds,
            visible: true,
            opacity: 85,
            globe_mesh: RefCell::default(),
        }
    }

    fn tint(&self, mut mesh: egui::Mesh) -> egui::Mesh {
        let tint = egui::Color32::from_white_alpha(((u16::from(self.opacity) * 255) / 100) as u8);
        for vertex in &mut mesh.vertices {
            vertex.color = tint;
        }
        mesh
    }

    fn center(&self) -> GeoPoint {
        let [west, south, east, north] = self.bounds;
        GeoPoint {
            lat: (south + north) * 0.5,
            lon: ((west + east) * 0.5 + 180.0).rem_euclid(360.0) - 180.0,
        }
    }

    pub(super) fn draw_globe(
        &self,
        painter: &egui::Painter,
        layout: &GlobeLayout,
        view: &GlobeViewState,
    ) {
        if !self.visible || self.opacity == 0 {
            return;
        }
        let key = [
            view.yaw,
            view.pitch,
            layout.radius,
            layout.focal_length,
            layout.camera_distance,
            layout.center.x,
            layout.center.y,
            f32::from(self.opacity),
        ]
        .map(f32::to_bits);
        let mut cache = self.globe_mesh.borrow_mut();
        if cache.as_ref().is_none_or(|(old, _)| old != &key) {
            let regional = self.bounds[2] - self.bounds[0] < 100.0;
            let mesh = raster_mesh::globe(layout, view, self.texture.id(), self.bounds, regional);
            *cache = Some((key, Arc::new(self.tint(mesh))));
        }
        if let Some((_, mesh)) = cache.as_ref() {
            painter.add(egui::Shape::mesh(mesh.clone()));
        }
    }

    pub(super) fn draw_local(
        &self,
        painter: &egui::Painter,
        layout: &LocalLayout,
        view: &GlobeViewState,
        focus: GeoPoint,
        half: f32,
    ) {
        if self.visible && self.opacity > 0 {
            let mesh = raster_mesh::local(
                layout,
                view,
                self.texture.id(),
                self.bounds,
                focus,
                half,
                painter.clip_rect(),
            );
            painter.add(egui::Shape::mesh(self.tint(mesh)));
        }
    }
}

pub(crate) fn controls(ui: &mut egui::Ui, model: &mut AppModel) {
    let mut remove = None;
    let mut center = None;
    for (index, layer) in model.satellite_layers.iter_mut().enumerate() {
        ui.push_id(("satellite-pass", index), |ui| {
            ui.horizontal(|ui| {
                ui.add_enabled(
                    model.active_body == ActiveBody::Earth,
                    egui::Checkbox::new(&mut layer.visible, &layer.name),
                )
                .on_disabled_hover_text("Earth imagery");
                if ui.small_button("×").on_hover_text("Remove image").clicked() {
                    remove = Some(index);
                }
            });
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        model.active_body == ActiveBody::Earth,
                        egui::Button::new("Center").small(),
                    )
                    .clicked()
                {
                    center = Some(layer.center());
                }
                ui.add(egui::Slider::new(&mut layer.opacity, 0..=100).suffix("%"))
                    .on_hover_text("Image opacity");
            });
        });
    }
    if let Some(index) = remove {
        model.satellite_layers.remove(index);
    }
    if let Some(center) = center {
        model.stop_event_follow();
        model.globe_view.stop_motion();
        model.globe_view.focus_on(center);
    }
}

#[cfg(test)]
#[path = "satellite_layer_tests.rs"]
mod tests;
