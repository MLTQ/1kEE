//! Compact map controls shared by normal and cinematic views.
use super::world_map::contour_asset;
use crate::{
    model::{ActiveBody, AppModel},
    theme,
};

#[derive(Clone, Copy, PartialEq)]
enum Density {
    Full,
    Compact,
    Minimal,
}

pub(super) fn render(ui: &mut egui::Ui, model: &mut AppModel) {
    egui::Frame::new().fill(theme::panel_fill(216))
        .stroke(egui::Stroke::new(1.0, theme::panel_stroke()))
        .corner_radius(10.0).inner_margin(egui::Margin::symmetric(12,8))
        .show(ui, |ui| {
            let density = if ui.available_width() >= 1150.0 { Density::Full }
                else if ui.available_width() >= 800.0 { Density::Compact } else { Density::Minimal };
            ui.horizontal(|ui| {
                view_buttons(ui, model);
                body_button(ui, model);
                ui.separator();
                let has_custom = !model.geojson_layers.is_empty() || !model.arcgis_sources.is_empty();
                if button(ui, if has_custom {"Layers •"} else {"Layers"}, model.show_layer_drawer)
                    .on_hover_text("Layer visibility and imported overlays").clicked() {
                    model.show_layer_drawer = !model.show_layer_drawer;
                }
                if density != Density::Minimal { record_buttons(ui, model); }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if button(ui, "Settings", model.settings_open)
                        .on_hover_text("Sources, appearance, files, Terrain Library and activity").clicked() {
                        model.settings_open = !model.settings_open;
                    }
                    let blast = egui::Button::new(egui::RichText::new("⚡ Cache Blast").small()
                        .color(egui::Color32::from_rgb(255,160,40)))
                        .fill(egui::Color32::from_rgba_premultiplied(80,40,0,180)).corner_radius(4.0);
                    if ui.add(blast).on_hover_text("Drop in-memory tile caches. Tiles reload from disk; no files are deleted.").clicked() {
                        contour_asset::blast_tile_caches();
                    }
                    match density {
                        Density::Minimal => {
                            ui.menu_button("View", |ui| {
                                record_buttons(ui, model);
                                ui.separator();
                                focus_button(ui, model);
                                cinematic_button(ui, model);
                                presentation_buttons(ui, model);
                            });
                        }
                        Density::Compact => {
                            focus_button(ui, model);
                            cinematic_button(ui, model);
                            ui.menu_button("More", |ui| presentation_buttons(ui, model));
                        }
                        Density::Full => {
                            focus_button(ui, model);
                            cinematic_button(ui, model);
                            presentation_buttons(ui, model);
                        }
                    }
                });
            });
        });
}
fn button(ui: &mut egui::Ui, label: &str, active: bool) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new(label).small().color(if active {
            theme::chrome_active_text()
        } else {
            theme::text_muted()
        }))
        .fill(if active {
            theme::chrome_active_fill()
        } else {
            egui::Color32::TRANSPARENT
        })
        .corner_radius(4.0),
    )
}
fn view_buttons(ui: &mut egui::Ui, model: &mut AppModel) {
    if button(ui, "GLOBE", !model.globe_view.local_mode).clicked() && model.globe_view.local_mode {
        model.stop_event_follow();
        model.globe_view.local_mode = false;
    }
    let disabled = match model.active_body {
        ActiveBody::Earth => false,
        ActiveBody::Moon => {
            crate::terrain_assets::find_sldem_jp2(model.selected_root.as_deref()).is_none()
        }
        ActiveBody::Mars => {
            crate::terrain_assets::find_mars_data_root(model.selected_root.as_deref()).is_none()
        }
    };
    ui.add_enabled_ui(!disabled, |ui| {
        if button(ui, "LOCAL", model.globe_view.local_mode).clicked()
            && !model.globe_view.local_mode
        {
            model.stop_event_follow();
            model.globe_view.local_center = model.globe_view.globe_center_latlon();
            model.globe_view.local_mode = true;
        }
    })
    .response
    .on_disabled_hover_text(match model.active_body {
        ActiveBody::Moon => "Local Moon view requires SLDEM2015 terrain data.",
        ActiveBody::Mars => "Local Mars view requires Mars terrain data.",
        ActiveBody::Earth => "Local terrain view",
    });
}
fn body_button(ui: &mut egui::Ui, model: &mut AppModel) {
    let (label, color, next) = match model.active_body {
        ActiveBody::Earth => ("EARTH", egui::Color32::from_rgb(210, 206, 194), "Moon"),
        ActiveBody::Moon => ("MOON", egui::Color32::from_rgb(126, 208, 229), "Mars"),
        ActiveBody::Mars => ("MARS", egui::Color32::from_rgb(238, 114, 91), "Earth"),
    };
    if ui
        .add(
            egui::Button::new(egui::RichText::new(label).small().color(color))
                .fill(theme::item_fill())
                .corner_radius(4.0),
        )
        .on_hover_text(format!("Switch to {next}"))
        .clicked()
    {
        model.stop_event_follow();
        model.active_body = match model.active_body {
            ActiveBody::Earth => ActiveBody::Moon,
            ActiveBody::Moon => ActiveBody::Mars,
            ActiveBody::Mars => ActiveBody::Earth,
        };
        if model.active_body != ActiveBody::Earth {
            model.globe_view.local_mode = false;
        }
        model.map_theme = match model.active_body {
            ActiveBody::Earth => theme::MapTheme::Topo,
            ActiveBody::Moon => theme::MapTheme::Lunar,
            ActiveBody::Mars => theme::MapTheme::MarsDark,
        };
        theme::set_theme(ui.ctx(), model.map_theme);
    }
}
fn record_buttons(ui: &mut egui::Ui, model: &mut AppModel) {
    if button(
        ui,
        &format!("Events {}", model.events.len()),
        model.show_event_list,
    )
    .on_hover_text("Show / hide event queue")
    .clicked()
    {
        model.show_event_list = !model.show_event_list;
    }
    if model.active_body == ActiveBody::Earth
        && button(
            ui,
            &format!("Cameras {}", model.cameras.len()),
            model.show_camera_markers,
        )
        .on_hover_text("Show / hide webcam markers")
        .clicked()
    {
        model.show_camera_markers = !model.show_camera_markers;
    }
}
fn focus_button(ui: &mut egui::Ui, model: &mut AppModel) {
    if button(ui, "Focus", false)
        .on_hover_text("Hide controls for capture. F10 toggles; Esc restores controls.")
        .clicked()
    {
        model.obs_view = true;
        ui.close_menu();
        ui.ctx().request_repaint();
    }
}
fn cinematic_button(ui: &mut egui::Ui, model: &mut AppModel) {
    let mut control = egui::Button::new(egui::RichText::new("CINEMATIC").small().color(
        if model.cinematic_mode {
            egui::Color32::from_rgb(255, 210, 80)
        } else {
            theme::text_muted()
        },
    ))
    .fill(egui::Color32::TRANSPARENT)
    .corner_radius(4.0);
    if model.cinematic_mode {
        control = control.fill(egui::Color32::from_rgb(160, 100, 20));
    }
    if ui
        .add(control)
        .on_hover_text("Enable cinematic camera controls")
        .clicked()
    {
        model.cinematic_mode = !model.cinematic_mode;
        if !model.cinematic_mode {
            model.stop_event_follow();
            model.globe_view.meander_mode = false;
        }
    }
}
fn presentation_buttons(ui: &mut egui::Ui, model: &mut AppModel) {
    if model.cinematic_mode {
        if model.active_body == ActiveBody::Earth && button(ui, "FOLLOW EVENTS", model.event_follow.enabled())
            .on_hover_text("Tour the six most severe Factal events; keeps Globe/Local view. Drag, scroll or arrows to stop.").clicked() {
            model.toggle_event_follow(); ui.ctx().request_repaint();
        }
        if button(ui, "MEANDER", model.globe_view.meander_mode)
            .on_hover_text("Smooth camera drift")
            .clicked()
        {
            let enable = !model.globe_view.meander_mode;
            if enable {
                model.event_follow.stop();
                model.globe_view.stop_motion();
            }
            model.globe_view.meander_mode = enable;
        }
        if model.globe_view.meander_mode {
            ui.scope(|ui| {
                ui.spacing_mut().slider_width = 60.0;
                ui.add(
                    egui::Slider::new(&mut model.globe_view.meander_speed, 0.05..=1.0)
                        .show_value(false),
                )
                .on_hover_text(format!(
                    "Meander speed {:.0}%",
                    model.globe_view.meander_speed * 100.0
                ));
            });
        }
    }
    if model.has_factal_api_key() && button(ui, "REPLAY", model.replay_mode).clicked() {
        model.toggle_replay();
    }
    if model.selected_event_has_factal_brief()
        && button(ui, "BRIEF", model.factal_brief_open).clicked()
    {
        model.factal_brief_open = true;
    }
    if !model.globe_view.local_mode
        && model.show_stellar_correspondence
        && button(ui, "Observatory", model.stellar_observatory_open).clicked()
    {
        model.stellar_observatory_open = !model.stellar_observatory_open;
    }
}
