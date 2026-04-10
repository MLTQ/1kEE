use crate::model::{ActiveBody, AppModel, GeoJsonLayer};
use crate::osm_ingest;
use crate::panels::world_map::contour_asset;
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

                if ui.button("Settings").clicked() {
                    model.factal_settings_open = true;
                }

                if ui.button("Terrain Library").clicked() {
                    model.terrain_library_open = true;
                }

                let body_label = match model.active_body {
                    ActiveBody::Earth => "EARTH",
                    ActiveBody::Moon => "MOON",
                    ActiveBody::Mars => "MARS",
                };
                let body_btn = egui::Button::new(
                    egui::RichText::new(body_label)
                        .color(match model.active_body {
                            ActiveBody::Earth => egui::Color32::from_rgb(210, 206, 194),
                            ActiveBody::Moon => egui::Color32::from_rgb(126, 208, 229),
                            ActiveBody::Mars => egui::Color32::from_rgb(238, 114, 91),
                        }),
                )
                .fill(match model.active_body {
                    ActiveBody::Earth => egui::Color32::from_rgba_premultiplied(28, 26, 22, 200),
                    ActiveBody::Moon => egui::Color32::from_rgba_premultiplied(10, 30, 45, 200),
                    ActiveBody::Mars => egui::Color32::from_rgba_premultiplied(35, 14, 11, 200),
                });
                if ui
                    .add(body_btn)
                    .on_hover_text(match model.active_body {
                        ActiveBody::Earth => "Switch to Moon Mode",
                        ActiveBody::Moon => "Switch to Mars Mode",
                        ActiveBody::Mars => "Switch back to Earth",
                    })
                    .clicked()
                {
                    model.active_body = match model.active_body {
                        ActiveBody::Earth => ActiveBody::Moon,
                        ActiveBody::Moon => ActiveBody::Mars,
                        ActiveBody::Mars => ActiveBody::Earth,
                    };
                    
                    // Non-Earth modes have no local terrain view — always return to globe.
                    if model.active_body != ActiveBody::Earth {
                        model.globe_view.local_mode = false;
                    }
                    let new_theme = match model.active_body {
                        ActiveBody::Earth => crate::theme::MapTheme::Topo,
                        ActiveBody::Moon => crate::theme::MapTheme::Lunar,
                        ActiveBody::Mars => crate::theme::MapTheme::MarsDark,
                    };
                    model.map_theme = new_theme;
                    crate::theme::set_theme(ui.ctx(), new_theme);
                }

                if ui.button("Import Layer").clicked() {
                    import_layer(model);
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

fn import_layer(model: &mut AppModel) {
    let Some(path) = rfd::FileDialog::new()
        .add_filter("Map layers", &["geojson", "json", "kml", "kmz"])
        .add_filter("GeoJSON", &["geojson", "json"])
        .add_filter("KML / KMZ", &["kml", "kmz"])
        .set_title("Import map layer")
        .pick_file()
    else {
        return;
    };

    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Imported layer".into());
    let format_label = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_uppercase())
        .unwrap_or_else(|| "LAYER".into());

    match std::fs::read(&path) {
        Err(e) => model.push_log(format!("Layer read error: {e}")),
        Ok(bytes) => match GeoJsonLayer::parse_upload(
            name.clone(),
            path.extension().and_then(|ext| ext.to_str()),
            &bytes,
        ) {
            Err(e) => model.push_log(format!("{format_label} parse error in \"{name}\": {e}")),
            Ok(layer) => {
                model.push_log(format!(
                    "{format_label} layer \"{name}\" loaded — {} feature(s).",
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
