use crate::arcgis_source;
use crate::model::AppModel;
use crate::theme;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum SidebarTab {
    #[default]
    Cameras,
    Items,
}

pub fn render_camera_list(ctx: &egui::Context, model: &mut AppModel) {
    egui::SidePanel::right("camera_list")
        .resizable(true)
        .min_width(300.0)
        .default_width(360.0)
        .max_width(420.0)
        .frame(egui::Frame::new().fill(theme::section_background()))
        .show(ctx, |ui| {
            // ── Tab bar ───────────────────────────────────────────────────
            let tab_id = ui.id().with("sidebar_tab");
            let mut tab: SidebarTab = ui.data(|d| d.get_temp(tab_id).unwrap_or_default());

            ui.horizontal(|ui| {
                for (t, label) in [
                    (SidebarTab::Cameras, "Cameras"),
                    (SidebarTab::Items, "Items"),
                ] {
                    let active = tab == t;
                    let fill = if active {
                        theme::chrome_active_fill()
                    } else {
                        egui::Color32::TRANSPARENT
                    };
                    let color = if active {
                        theme::chrome_active_text()
                    } else {
                        theme::text_muted()
                    };
                    if ui
                        .add(
                            egui::Button::new(egui::RichText::new(label).color(color))
                                .fill(fill)
                                .corner_radius(4.0),
                        )
                        .clicked()
                    {
                        tab = t;
                    }
                }
            });
            ui.separator();

            match tab {
                SidebarTab::Cameras => tab_cameras(ui, model),
                SidebarTab::Items => tab_items(ui, model),
            }

            ui.data_mut(|d| d.insert_temp(tab_id, tab));
        });
}

// ── Cameras tab ───────────────────────────────────────────────────────────────

fn tab_cameras(ui: &mut egui::Ui, model: &mut AppModel) {
    ui.heading("Nearby Cameras");
    ui.colored_label(
        theme::text_muted(),
        "Openly published camera records near the selected event.",
    );
    ui.add_space(8.0);

    let nearby = model.nearby_cameras(250.0);

    if nearby.is_empty() {
        ui.label("Select an event to inspect nearby cameras.");
        return;
    }

    if let Some(event) = model.selected_event() {
        ui.small(format!(
            "{} cameras within 250 km of {}",
            nearby.len(),
            event.location_name
        ));
    }

    ui.add_space(8.0);

    egui::ScrollArea::vertical().show(ui, |ui| {
        for camera in nearby {
            let is_selected = model.selected_camera_id.as_deref() == Some(camera.id.as_str());

            egui::Frame::group(ui.style())
                .fill(if is_selected {
                    theme::selected_item_fill()
                } else {
                    theme::item_fill()
                })
                .inner_margin(egui::Margin::same(12))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.strong(camera.label.as_str());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.colored_label(camera.status.color(), camera.status.label());
                        });
                    });
                    ui.label(format!(
                        "{} | {} | {:.1} km",
                        camera.provider, camera.kind, camera.distance_km
                    ));
                    ui.small(format!("Last seen: {}", camera.last_seen));
                    ui.small(camera.stream_url.as_str());
                    ui.horizontal(|ui| {
                        if ui.button("Select").clicked() {
                            model.select_camera(&camera.id);
                        }
                        if ui.button("Attempt feed").clicked() {
                            model.attempt_connect(&camera.id);
                        }
                    });
                });

            ui.add_space(8.0);
        }
    });
}

// ── Items tab ─────────────────────────────────────────────────────────────────

fn tab_items(ui: &mut egui::Ui, model: &mut AppModel) {
    ui.heading("Map Items");
    ui.colored_label(
        theme::text_muted(),
        "ArcGIS FeatureServer sources — paste any URL to add a new source.",
    );
    ui.add_space(8.0);

    // URL input
    let url_id = ui.id().with("arcgis_url_input");
    let mut url_buf: String = ui.data(|d| d.get_temp(url_id).unwrap_or_default());

    ui.horizontal(|ui| {
        let input_width = (ui.available_width() - 96.0).max(140.0);
        let te = ui.add(
            egui::TextEdit::singleline(&mut url_buf)
                .hint_text("ArcGIS FeatureServer URL\u{2026}")
                .desired_width(input_width),
        );
        let add_clicked = ui.button("Add Source").clicked();
        if (add_clicked || (te.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))))
            && !url_buf.trim().is_empty()
        {
            let canonical = arcgis_source::normalize_url(&url_buf);
            if !model
                .arcgis_sources
                .iter()
                .any(|s| arcgis_source::normalize_url(&s.url) == canonical)
            {
                let color_offset = model.arcgis_sources.len() * 2;
                arcgis_source::add_source(canonical.clone(), color_offset, ui.ctx().clone());
                model.arcgis_sources.push(crate::model::ArcGisSourceRef {
                    url: canonical,
                    enabled_layer_ids: std::collections::HashSet::new(),
                });
                url_buf.clear();
            }
        }
        ui.data_mut(|d| d.insert_temp(url_id, url_buf.clone()));
    });

    ui.add_space(6.0);

    // Source list
    let mut to_remove: Option<usize> = None;
    egui::ScrollArea::vertical().show(ui, |ui| {
        for (si, src_ref) in model.arcgis_sources.iter_mut().enumerate() {
            let snap = arcgis_source::source_snapshot(&src_ref.url);
            let source_title = if let Some(ref s) = snap {
                if s.discovering {
                    "Discovering…".to_owned()
                } else if let Some(ref e) = s.discover_error {
                    format!("Error: {e}")
                } else {
                    let layer_count = s.layers.as_ref().map(|l| l.len()).unwrap_or(0);
                    format!(
                        "{} ({} layer{})",
                        s.display_name,
                        layer_count,
                        if layer_count == 1 { "" } else { "s" }
                    )
                }
            } else {
                src_ref.url.clone()
            };
            let source_color = match snap.as_ref() {
                Some(s) if s.discover_error.is_some() => egui::Color32::from_rgb(200, 80, 80),
                _ => ui.visuals().text_color(),
            };

            ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.vertical(|ui| {
                    if ui.small_button("\u{2715}").clicked() {
                        to_remove = Some(si);
                    }
                });
                ui.add_sized(
                    [ui.available_width(), 0.0],
                    egui::Label::new(egui::RichText::new(source_title).color(source_color)).wrap(),
                );
            });

            // Layer checkboxes
            if let Some(ref s) = snap {
                if let Some(ref layers) = s.layers {
                    for layer in layers {
                        let mut enabled = src_ref.enabled_layer_ids.contains(&layer.id);
                        ui.horizontal(|ui| {
                            ui.add_space(12.0);
                            // Color swatch
                            let (rect, _) =
                                ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                            ui.painter().circle_filled(rect.center(), 4.0, layer.color);

                            ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                                let status =
                                    s.layer_status.get(&layer.id).cloned().unwrap_or_default();
                                if !status.is_empty() {
                                    ui.colored_label(theme::text_muted(), status);
                                }

                                let changed = ui
                                    .add_sized(
                                        [ui.available_width(), 0.0],
                                        egui::Checkbox::new(&mut enabled, &layer.name),
                                    )
                                    .changed();
                                if changed {
                                    if enabled {
                                        src_ref.enabled_layer_ids.insert(layer.id);
                                    } else {
                                        src_ref.enabled_layer_ids.remove(&layer.id);
                                    }
                                }
                            });
                        });
                    }
                }
            }

            ui.separator();
        }
    });

    if let Some(i) = to_remove {
        let url = model.arcgis_sources[i].url.clone();
        arcgis_source::remove_source(&url);
        model.arcgis_sources.remove(i);
    }

    // Footer count
    let total = model.arcgis_features.len();
    if total > 0 {
        ui.label(
            egui::RichText::new(format!("{total} features loaded"))
                .small()
                .color(theme::text_muted()),
        );
    }
}
