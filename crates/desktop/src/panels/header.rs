use crate::model::{AppModel, GeoJsonLayer};
use crate::osm_ingest;
use crate::terrain_assets;
use crate::theme;
use crate::panels::world_map::contour_asset;

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

                if ui.button("Settings").clicked() {
                    model.factal_settings_open = true;
                }

                if ui.button("Terrain Library").clicked() {
                    model.terrain_library_open = true;
                }

                if ui.button("Import GeoJSON").clicked() {
                    import_geojson(model);
                }

                let blast_btn = egui::Button::new(
                    egui::RichText::new("⚡ Cache Blast")
                        .color(egui::Color32::from_rgb(255, 160, 40)),
                )
                .fill(egui::Color32::from_rgba_premultiplied(80, 40, 0, 180));
                if ui.add(blast_btn)
                    .on_hover_text("Instantly drop all in-memory tile caches (global + regional).\nTiles re-read from disk on next frame — nothing is deleted.")
                    .clicked()
                {
                    contour_asset::blast_tile_caches();
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

fn import_geojson(model: &mut AppModel) {
    let Some(path) = rfd::FileDialog::new()
        .add_filter("GeoJSON", &["geojson", "json"])
        .set_title("Import GeoJSON layer")
        .pick_file()
    else {
        return;
    };

    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "GeoJSON layer".into());

    match std::fs::read_to_string(&path) {
        Err(e) => model.push_log(format!("GeoJSON read error: {e}")),
        Ok(text) => match GeoJsonLayer::parse(name.clone(), &text) {
            Err(e) => model.push_log(format!("GeoJSON parse error in \"{name}\": {e}")),
            Ok(layer) => {
                model.push_log(format!(
                    "GeoJSON layer \"{name}\" loaded — {} feature(s).",
                    layer.features.len()
                ));
                model.geojson_layers.push(layer);
            }
        },
    }
}

fn metric_chip(ui: &mut egui::Ui, label: &str, value: &str) {
    egui::Frame::new()
        .fill(theme::item_fill())
        .corner_radius(8.0)
        .inner_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.colored_label(theme::text_muted(), label);
                ui.strong(value);
            });
        });
}
