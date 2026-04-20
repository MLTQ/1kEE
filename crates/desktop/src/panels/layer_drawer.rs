use crate::arcgis_source;
use crate::model::AppModel;
use crate::panels::world_map;
use crate::theme;

pub fn render_layer_drawer(ctx: &egui::Context, model: &mut AppModel) {
    if !model.show_layer_drawer {
        return;
    }

    egui::SidePanel::right("layer_drawer")
        .resizable(false)
        .exact_width(230.0)
        .frame(egui::Frame::new().fill(theme::section_background()))
        .show(ctx, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.strong("Layers");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("✕").clicked() {
                        model.show_layer_drawer = false;
                    }
                });
            });
            ui.add_space(4.0);
            ui.separator();

            egui::ScrollArea::vertical().show(ui, |ui| {
                // ── BASE ─────────────────────────────────────────────────────
                section_label(ui, "BASE");
                ui.checkbox(&mut model.show_event_markers, "Events");
                ui.checkbox(&mut model.show_coastlines, "Coastline");
                ui.checkbox(&mut model.show_bathymetry, "Bathymetry");
                ui.checkbox(&mut model.show_graticule, "Graticule");
                ui.checkbox(&mut model.show_reticle, "Reticle");
                ui.checkbox(&mut model.fill_elevation, "Terrain fill");
                {
                    let ships_enabled = !model.aisstream_api_key.is_empty();
                    ui.add_enabled(
                        ships_enabled,
                        egui::Checkbox::new(&mut model.show_ships, "Ships"),
                    )
                    .on_disabled_hover_text("Configure AISStream key in Settings");
                }
                ui.checkbox(&mut model.show_flights, "Flights");
                ui.checkbox(&mut model.show_stellar_correspondence, "Stars");
                if model.show_stellar_correspondence && !model.globe_view.local_mode {
                    let obs_active = model.stellar_observatory_open;
                    let fill = if obs_active { theme::chrome_active_fill() } else { egui::Color32::TRANSPARENT };
                    let col  = if obs_active { theme::chrome_active_text() } else { theme::text_muted() };
                    ui.indent("obs_indent", |ui| {
                        if ui.add(egui::Button::new(egui::RichText::new("Observatory").small().color(col)).fill(fill).corner_radius(4.0)).clicked() {
                            model.stellar_observatory_open = !model.stellar_observatory_open;
                        }
                    });
                }
                ui.add_space(6.0);

                // ── TRANSPORT ────────────────────────────────────────────────
                section_label(ui, "TRANSPORT");
                let major_changed = ui.checkbox(&mut model.show_major_roads, "Major roads").changed();
                let minor_changed = ui.checkbox(&mut model.show_minor_roads, "Minor roads").changed();
                if (major_changed || minor_changed) && !model.show_major_roads && !model.show_minor_roads {
                    world_map::invalidate_road_cache_pub();
                }
                ui.checkbox(&mut model.show_rail, "Railways");
                ui.checkbox(&mut model.show_aeroway, "Airports");
                ui.add_space(6.0);

                // ── NATURAL ──────────────────────────────────────────────────
                section_label(ui, "NATURAL");
                let water_changed = ui.checkbox(&mut model.show_water, "Water").changed();
                if water_changed && !model.show_water {
                    world_map::invalidate_water_cache_pub();
                }
                ui.checkbox(&mut model.show_contours, "Contours");
                ui.checkbox(&mut model.show_trees, "Trees");
                ui.add_space(6.0);

                // ── URBAN ────────────────────────────────────────────────────
                section_label(ui, "URBAN");
                ui.checkbox(&mut model.show_buildings, "Buildings");
                ui.checkbox(&mut model.show_admin, "Admin Boundaries");
                ui.checkbox(&mut model.show_military, "Military");
                ui.checkbox(&mut model.show_industrial, "Industrial");
                ui.checkbox(&mut model.show_port, "Ports");
                ui.checkbox(&mut model.show_government, "Government");
                ui.add_space(6.0);

                // ── INFRASTRUCTURE ───────────────────────────────────────────
                section_label(ui, "INFRASTRUCTURE");
                ui.checkbox(&mut model.show_power, "Power lines");
                ui.checkbox(&mut model.show_pipeline, "Pipelines");
                ui.checkbox(&mut model.show_comm, "Comms towers");
                ui.checkbox(&mut model.show_surveillance, "Surveillance");
                ui.add_space(6.0);

                // ── IMPORTED LAYERS ──────────────────────────────────────────
                if !model.geojson_layers.is_empty() {
                    section_label(ui, "IMPORTED LAYERS");
                    let mut remove_idx: Option<usize> = None;
                    for (idx, layer) in model.geojson_layers.iter_mut().enumerate() {
                        let [r, g, b, _] = layer.color;
                        let dot_color = egui::Color32::from_rgb(r, g, b);
                        ui.horizontal(|ui| {
                            let (rect, _) = ui.allocate_exact_size(
                                egui::vec2(8.0, 8.0),
                                egui::Sense::hover(),
                            );
                            ui.painter().circle_filled(rect.center(), 4.0, dot_color);
                            ui.checkbox(&mut layer.visible, &layer.name);
                            if ui.small_button("×").on_hover_text("Remove").clicked() {
                                remove_idx = Some(idx);
                            }
                        });
                    }
                    if let Some(idx) = remove_idx {
                        model.geojson_layers.remove(idx);
                    }
                    ui.add_space(6.0);
                }

                // ── ARCGIS SOURCES ───────────────────────────────────────────
                section_label(ui, "ARCGIS SOURCES");
                let url_id = ui.id().with("arcgis_url_input");
                let mut url_buf: String = ui.data(|d| d.get_temp(url_id).unwrap_or_default());
                ui.horizontal(|ui| {
                    let te = ui.add(
                        egui::TextEdit::singleline(&mut url_buf)
                            .hint_text("FeatureServer URL…")
                            .desired_width(ui.available_width() - 52.0),
                    );
                    let add = ui.small_button("Add").clicked();
                    if (add || (te.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))))
                        && !url_buf.trim().is_empty()
                    {
                        let canonical = arcgis_source::normalize_url(&url_buf);
                        if !model.arcgis_sources.iter().any(|s| arcgis_source::normalize_url(&s.url) == canonical) {
                            let color_offset = model.arcgis_sources.len() * 2;
                            arcgis_source::add_source(canonical.clone(), color_offset, ui.ctx().clone());
                            model.arcgis_sources.push(crate::model::ArcGisSourceRef {
                                url: canonical,
                                enabled_layer_ids: std::collections::HashSet::new(),
                            });
                            url_buf.clear();
                        }
                    }
                    ui.data_mut(|d| d.insert_temp(url_id, url_buf));
                });

                ui.add_space(4.0);

                let mut to_remove: Option<usize> = None;
                for (si, src_ref) in model.arcgis_sources.iter_mut().enumerate() {
                    let snap = arcgis_source::source_snapshot(&src_ref.url);
                    let title = snap.as_ref().map_or_else(
                        || src_ref.url.clone(),
                        |s| {
                            if s.discovering { "Discovering…".into() }
                            else if let Some(ref e) = s.discover_error { format!("Error: {e}") }
                            else { s.display_name.clone() }
                        },
                    );
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(&title).small().color(theme::text_muted()));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("×").clicked() {
                                to_remove = Some(si);
                            }
                        });
                    });
                    if let Some(ref s) = snap {
                        if let Some(ref layers) = s.layers {
                            for layer in layers {
                                let mut enabled = src_ref.enabled_layer_ids.contains(&layer.id);
                                ui.horizontal(|ui| {
                                    ui.add_space(8.0);
                                    let (rect, _) = ui.allocate_exact_size(egui::vec2(6.0, 6.0), egui::Sense::hover());
                                    ui.painter().circle_filled(rect.center(), 3.0, layer.color);
                                    if ui.checkbox(&mut enabled, &layer.name).changed() {
                                        if enabled { src_ref.enabled_layer_ids.insert(layer.id); }
                                        else { src_ref.enabled_layer_ids.remove(&layer.id); }
                                    }
                                });
                            }
                        }
                    }
                    ui.add_space(2.0);
                }
                if let Some(i) = to_remove {
                    let url = model.arcgis_sources[i].url.clone();
                    arcgis_source::remove_source(&url);
                    model.arcgis_sources.remove(i);
                }
            });
        });
}

fn section_label(ui: &mut egui::Ui, label: &str) {
    ui.colored_label(theme::text_muted(), egui::RichText::new(label).small());
    ui.add_space(2.0);
}
