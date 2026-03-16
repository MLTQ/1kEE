mod camera;
mod contour_asset;
mod globe_scene;
mod local_terrain_scene;
pub(crate) mod srtm_focus_cache;
mod srtm_stream;
mod terrain_field;
mod terrain_raster;

use crate::model::AppModel;
use crate::theme;

pub fn render_world_map(ui: &mut egui::Ui, model: &mut AppModel) {
    let panel_frame = egui::Frame::new()
        .fill(theme::section_background())
        .corner_radius(12.0)
        .inner_margin(egui::Margin::same(14));

    panel_frame.show(ui, |ui| {
        if model.globe_view.auto_spin {
            ui.ctx().request_repaint();
        } else {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(96));
        }

        ui.horizontal(|ui| {
            ui.heading("Operations Globe");
            ui.separator();
            ui.colored_label(
                theme::text_muted(),
                "3D-first tactical globe with drag orbit, wheel zoom, and contour-driven LOD.",
            );
        });

        ui.add_space(8.0);

        let local_terrain_mode = local_terrain_scene::is_active(model);
        let footer_height = if local_terrain_mode { 72.0 } else { 0.0 };
        let desired = egui::vec2(
            ui.available_width().max(480.0),
            (ui.available_height() - footer_height).max(360.0),
        );
        let (response, painter) = ui.allocate_painter(desired, egui::Sense::click_and_drag());
        let rect = response.rect;

        camera::apply_interaction(ui.ctx(), &response, &mut model.globe_view);
        let transition_progress = local_terrain_scene::transition_progress(model.globe_view.zoom);
        let scene = if local_terrain_mode {
            local_terrain_scene::paint(&painter, rect, model, ui.ctx().input(|input| input.time))
        } else {
            let scene =
                globe_scene::paint(&painter, rect, model, ui.ctx().input(|input| input.time));
            if transition_progress > 0.0 {
                local_terrain_scene::paint_transition_overlay(
                    &painter,
                    rect,
                    model,
                    transition_progress,
                );
            }
            scene
        };

        if model.terrain_focus_location().is_some() {
            draw_focus_card(ui, model, local_terrain_mode);
        }
        if local_terrain_mode {
            ui.add_space(10.0);
            draw_local_footer(ui, model);
        }

        if response.clicked() && response.drag_delta().length_sq() < 4.0 {
            if let Some(pointer) = response.interact_pointer_pos() {
                if let Some((camera_id, _)) = scene
                    .camera_markers
                    .iter()
                    .find(|(_, marker)| marker.distance(pointer) <= 9.0)
                {
                    model.select_camera(camera_id);
                } else if let Some((event_id, _)) = scene
                    .event_markers
                    .iter()
                    .find(|(_, marker)| marker.distance(pointer) <= 11.0)
                {
                    model.select_event(event_id);
                }
            }
        }
    });
}

fn draw_focus_card(ui: &mut egui::Ui, model: &AppModel, local_terrain_mode: bool) {
    egui::Area::new("focus_card".into())
        .fixed_pos(ui.min_rect().left_top() + egui::vec2(22.0, 72.0))
        .interactable(false)
        .show(ui.ctx(), |ui| {
            egui::Frame::new()
                .fill(egui::Color32::from_rgba_premultiplied(7, 18, 24, 230))
                .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(24, 63, 79)))
                .corner_radius(10.0)
                .inner_margin(egui::Margin::same(10))
                .show(ui, |ui| {
                    ui.colored_label(
                        theme::hot_color(),
                        if local_terrain_mode {
                            "LOCAL / 3D CONTOUR STACK"
                        } else {
                            "3D / DARK TOPO / WIREFRAME"
                        },
                    );
                    if let Some(severity) = model.terrain_focus_severity() {
                        ui.colored_label(severity.color(), severity.label());
                    } else {
                        ui.colored_label(theme::topo_color(), "City");
                    }
                    ui.strong(model.terrain_focus_title());
                    ui.label(model.terrain_focus_location_name());
                    ui.small(format!("Source: {}", model.terrain_focus_source()));
                    ui.small(if local_terrain_mode {
                        "Drag to pan | Ctrl/Shift-drag to rotate | scroll to zoom"
                    } else {
                        "Drag to orbit | scroll to zoom"
                    });
                });
        });
}

fn draw_local_footer(ui: &mut egui::Ui, model: &mut AppModel) {
    egui::Frame::new()
        .fill(egui::Color32::from_rgba_premultiplied(7, 18, 24, 216))
        .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(24, 63, 79)))
        .corner_radius(10.0)
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(theme::topo_color(), "LAYER SPREAD");
                ui.add_sized(
                    [220.0, 18.0],
                    egui::Slider::new(&mut model.globe_view.local_layer_spread, 0.15..=1.6)
                        .text("Compress / expand")
                        .show_value(true),
                );

                ui.separator();
                ui.colored_label(theme::hot_color(), "ORANGE");
                ui.label("major contours (50m)");

                ui.separator();
                ui.colored_label(theme::topo_color(), "BLUE");
                ui.label("minor contours");

                if local_terrain_scene::is_active(model) {
                    ui.separator();
                    ui.label(format!("Terrain zoom {:.1}x", model.globe_view.zoom));
                }
            });
        });
}
