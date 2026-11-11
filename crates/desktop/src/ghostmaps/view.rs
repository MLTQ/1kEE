use super::{Job, PROJECT_URL, Source, export_name};
use crate::model::AppModel;

pub(crate) fn controls(ui: &mut egui::Ui, model: &mut AppModel) {
    let report_count = model
        .events
        .iter()
        .filter(|event| event.is_ghostmaps())
        .count();
    let mut locate = false;
    let source = &mut model.ghostmaps;
    let mut changed = ui
        .checkbox(&mut source.settings.enabled, "GhostMaps events")
        .on_hover_text("S2 Underground · checks for updated KMZ exports daily while enabled")
        .changed();
    if source.settings.enabled {
        ui.indent("ghostmaps_controls", |ui| {
            if let Some((job, _)) = &source.worker {
                match job {
                    Job::Catalog => ui.small("Checking GhostMaps catalog…"),
                    Job::Export { path, .. } => ui.small(format!("Loading {}…", export_name(path))),
                };
            }
            if let Some(c) = &source.catalog {
                ui.small(format!(
                    "Catalog checked {} UTC",
                    crate::event_store::unix_to_datetime_str(c.checked as i64)
                ));
            }
            if let Some(warning) = &source.catalog_warning {
                ui.small(format!("{warning} · retry in 1h"));
            }
            if report_count > 0 {
                ui.small(format!("{report_count} reports in Event Queue"));
                locate = ui
                    .add_enabled(
                        model.active_body == crate::model::ActiveBody::Earth,
                        egui::Button::new("Show nearest report").small(),
                    )
                    .on_hover_text(
                        "Center the Earth map on the GhostMaps report closest to the current view",
                    )
                    .clicked();
            }
            changed |= choose_exports(ui, source);
            changed |= ui
                .checkbox(&mut source.settings.labels, "Show placemark labels")
                .changed();
            for path in &source.settings.selected {
                ui.push_id(path, |ui| {
                    ui.label(export_name(path)).on_hover_text(path);
                    if let Some(product) = source.products.get(path) {
                        if product.layers.is_empty() {
                            ui.small("No placemarks in this export");
                        }
                        for layer in &product.layers {
                            let mut visible = layer.visible;
                            let [r, g, b, _] = layer.color;
                            let label = format!("{} ({})", layer.name, layer.features.len());
                            if ui
                                .checkbox(
                                    &mut visible,
                                    egui::RichText::new(label)
                                        .color(egui::Color32::from_rgb(r, g, b)),
                                )
                                .changed()
                            {
                                let hidden = source
                                    .settings
                                    .hidden_folders
                                    .entry(path.clone())
                                    .or_default();
                                if visible {
                                    hidden.remove(&layer.name);
                                } else {
                                    hidden.insert(layer.name.clone());
                                }
                                changed = true;
                            }
                        }
                    }
                    if let Some((_, error)) = source.errors.get(path) {
                        ui.small(format!("{error} · retry in 1h"));
                    } else if source
                        .catalog
                        .as_ref()
                        .is_some_and(|c| !c.files.contains_key(path))
                    {
                        ui.small("No longer in the published catalog");
                    }
                });
            }
            ui.horizontal_wrapped(|ui| {
                ui.hyperlink_to("S2 Underground / S2A1", PROJECT_URL);
                ui.hyperlink_to(
                    "CC BY-NC-SA 4.0",
                    "https://creativecommons.org/licenses/by-nc-sa/4.0/",
                );
            });
        });
    }
    if changed {
        source
            .products
            .retain(|p, _| source.settings.selected.contains(p));
        source.apply_visibility();
        model.sync_ghostmaps_reports();
        if let Err(error) = model.save_settings() {
            model.push_log(format!("Could not save GhostMaps settings: {error}"));
        }
    }
    if locate {
        let center = if model.globe_view.local_mode {
            model.globe_view.local_center
        } else {
            model.globe_view.globe_center_latlon()
        };
        let nearest = model
            .events
            .iter()
            .filter(|e| e.is_ghostmaps())
            .map(|e| (crate::model::haversine_km(center, e.location), e))
            .min_by(|(a, _), (b, _)| a.total_cmp(b));
        if let Some((_, event)) = nearest {
            let (id, location) = (event.id.clone(), event.location);
            if model.replay_mode {
                model.toggle_replay();
            }
            model.show_event_markers = true;
            model.select_event(&id);
            // A previously selected report may have been panned out of view.
            model.globe_view.focus_on(location);
        }
    }
}

fn choose_exports(ui: &mut egui::Ui, source: &mut Source) -> bool {
    let Some(catalog) = &source.catalog else {
        return false;
    };
    let mut changed = false;
    ui.collapsing("Choose KMZ exports", |ui| {
        ui.add(
            egui::TextEdit::singleline(&mut source.search).hint_text("Filter by name or folder"),
        );
        let query = source.search.trim().to_lowercase().replace('_', " ");
        let missing = source
            .settings
            .selected
            .iter()
            .filter(|path| !catalog.files.contains_key(*path))
            .cloned()
            .collect::<Vec<_>>();
        egui::ScrollArea::vertical()
            .id_salt("ghostmaps_exports")
            .max_height(240.0)
            .show(ui, |ui| {
                for path in missing.iter().chain(catalog.files.keys()) {
                    if !path.to_lowercase().replace('_', " ").contains(&query) {
                        continue;
                    }
                    let mut selected = source.settings.selected.contains(path);
                    if ui
                        .push_id(path, |ui| {
                            ui.checkbox(&mut selected, export_name(path))
                                .on_hover_text(path)
                        })
                        .inner
                        .changed()
                    {
                        if selected {
                            source.settings.selected.insert(path.clone());
                        } else {
                            source.settings.selected.remove(path);
                        }
                        changed = true;
                    }
                }
            });
    });
    changed
}
