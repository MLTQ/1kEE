use crate::model::AppModel;
use crate::osm_ingest;
use crate::terrain_assets;
use crate::theme;

pub fn render_header(ctx: &egui::Context, model: &mut AppModel) {
    egui::TopBottomPanel::top("header")
        .resizable(false)
        .frame(egui::Frame::new().fill(theme::section_background()))
        .show(ctx, |ui| {
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                ui.vertical(|ui| {
                    ui.heading("1kEE");
                    ui.colored_label(
                        theme::text_muted(),
                        "One Thousand Electric Eye | global event-to-camera operations demo",
                    );
                });

                ui.separator();

                metric_chip(ui, "Factal stream", &model.factal_stream_status);
                metric_chip(ui, "Camera registry", &model.camera_registry_status);
                metric_chip(ui, "Terrain", model.terrain_inventory.status_label());
                metric_chip(ui, "OSM", model.osm_inventory.status_label());
                metric_chip(ui, "Events", &model.events.len().to_string());
                metric_chip(ui, "Cameras", &model.cameras.len().to_string());

                if ui.button("Factal API").clicked() {
                    model.factal_settings_open = true;
                }

                if ui.button("Pick Data Root").clicked() {
                    if let Some(path) = rfd::FileDialog::new()
                        .set_directory(
                            model
                                .selected_root
                                .clone()
                                .unwrap_or_else(|| std::env::current_dir().unwrap_or_default()),
                        )
                        .pick_folder()
                    {
                        model.set_selected_root(path);
                    }
                }

                if ui.button("Terrain Library").clicked() {
                    model.terrain_library_open = true;
                }

                if model.terrain_focus_location().is_some() {
                    ui.separator();
                    ui.label(format!("Focus: {}", model.terrain_focus_location_name()));
                }

                ui.separator();
                ui.colored_label(
                    theme::text_muted(),
                    model.terrain_inventory.primary_runtime_source,
                );

                ui.separator();
                ui.colored_label(
                    theme::text_muted(),
                    model.osm_inventory.primary_runtime_source,
                );

                if let Some(root) = terrain_assets::find_srtm_root(model.selected_root.as_deref()) {
                    ui.colored_label(theme::text_muted(), format!("SRTM {}", root.display()));
                } else if let Some(planet) =
                    osm_ingest::find_planet_pbf(model.selected_root.as_deref())
                {
                    ui.colored_label(theme::text_muted(), format!("OSM {}", planet.display()));
                } else if let Some(root) = &model.selected_root {
                    ui.colored_label(theme::text_muted(), root.display().to_string());
                }
            });
            ui.add_space(4.0);
        });
}

fn metric_chip(ui: &mut egui::Ui, label: &str, value: &str) {
    egui::Frame::new()
        .fill(egui::Color32::from_rgb(13, 30, 43))
        .corner_radius(8.0)
        .inner_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.colored_label(theme::text_muted(), label);
                ui.strong(value);
            });
        });
}
