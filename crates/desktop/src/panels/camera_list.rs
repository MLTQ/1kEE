use crate::model::AppModel;
use crate::s2_underground;
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
        .default_width(360.0)
        .frame(egui::Frame::new().fill(theme::section_background()))
        .show(ctx, |ui| {
            // ── Tab bar ───────────────────────────────────────────────────
            let tab_id = ui.id().with("sidebar_tab");
            let mut tab: SidebarTab = ui.data(|d| d.get_temp(tab_id).unwrap_or_default());

            ui.horizontal(|ui| {
                for (t, label) in [
                    (SidebarTab::Cameras, "Cameras"),
                    (SidebarTab::Items,   "Items"),
                ] {
                    let active = tab == t;
                    let fill  = if active { theme::chrome_active_fill() } else { egui::Color32::TRANSPARENT };
                    let color = if active { theme::chrome_active_text() } else { theme::text_muted() };
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
                SidebarTab::Items   => tab_items(ui, model),
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
            let is_selected =
                model.selected_camera_id.as_deref() == Some(camera.id.as_str());

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
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                ui.colored_label(camera.status.color(), camera.status.label());
                            },
                        );
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
        "S2Underground open-access intelligence layers.",
    );
    ui.add_space(2.0);
    ui.small(
        egui::RichText::new("Data: S2Underground \u{00B7} CC BY-NC-SA 4.0")
            .color(theme::text_muted()),
    );
    ui.add_space(8.0);

    egui::ScrollArea::vertical().show(ui, |ui| {
        for layer in s2_underground::LAYERS {
            let col = egui::Color32::from_rgb(layer.color.0, layer.color.1, layer.color.2);
            let enabled = model
                .s2_layer_enabled
                .entry(layer.key.to_owned())
                .or_insert(false);

            egui::Frame::new()
                .fill(if *enabled {
                    theme::selected_item_fill()
                } else {
                    theme::item_fill()
                })
                .corner_radius(6.0)
                .inner_margin(egui::Margin::symmetric(10, 8))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        // Colour swatch dot
                        let (rect, _) = ui.allocate_exact_size(
                            egui::vec2(10.0, 10.0),
                            egui::Sense::hover(),
                        );
                        ui.painter().circle_filled(rect.center(), 5.0, col);

                        ui.checkbox(enabled, layer.display_name);

                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                let status = s2_underground::layer_status(layer.key);
                                ui.small(
                                    egui::RichText::new(&status).color(
                                        if status.starts_with("error") || status.starts_with("HTTP") {
                                            egui::Color32::from_rgb(210, 80, 80)
                                        } else if status == "loading\u{2026}" {
                                            theme::text_muted()
                                        } else {
                                            col.gamma_multiply(0.85)
                                        },
                                    ),
                                );
                            },
                        );
                    });
                });

            ui.add_space(4.0);
        }

        // Summary count of total loaded events
        let total: usize = s2_underground::LAYERS
            .iter()
            .filter(|l| model.s2_layer_enabled.get(l.key).copied().unwrap_or(false))
            .map(|l| {
                let status = s2_underground::layer_status(l.key);
                // Parse "N events" from status
                status
                    .split_whitespace()
                    .next()
                    .and_then(|n| n.parse::<usize>().ok())
                    .unwrap_or(0)
            })
            .sum();

        if total > 0 {
            ui.add_space(8.0);
            ui.separator();
            ui.add_space(4.0);
            ui.colored_label(theme::text_muted(), format!("{total} total events loaded"));
        }
    });
}
