use crate::model::AppModel;
use crate::theme;

pub fn render_camera_list(ctx: &egui::Context, model: &mut AppModel) {
    egui::SidePanel::right("camera_list")
        .resizable(true)
        .default_width(360.0)
        .frame(egui::Frame::new().fill(theme::section_background()))
        .show(ctx, |ui| {
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
                                        ui.colored_label(
                                            camera.status.color(),
                                            camera.status.label(),
                                        );
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
        });
}
