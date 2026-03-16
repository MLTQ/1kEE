use crate::factal_stream;
use crate::model::AppModel;
use crate::settings_store;
use crate::theme;

pub fn render_factal_settings(ctx: &egui::Context, model: &mut AppModel) {
    if !model.factal_settings_open {
        return;
    }

    let mut open = model.factal_settings_open;
    let mut save_requested = false;
    let mut clear_requested = false;
    let mut poll_requested = false;

    egui::Window::new("Factal API")
        .open(&mut open)
        .default_size(egui::vec2(480.0, 220.0))
        .min_size(egui::vec2(420.0, 200.0))
        .frame(
            egui::Frame::window(&ctx.style())
                .fill(egui::Color32::from_rgb(14, 18, 23))
                .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(43, 49, 58))),
        )
        .show(ctx, |ui| {
            ui.colored_label(
                theme::text_muted(),
                "Configure the private Factal token used for live event polling every minute.",
            );
            ui.add_space(10.0);

            ui.label("API Key");
            ui.add_sized(
                [ui.available_width(), 30.0],
                egui::TextEdit::singleline(&mut model.factal_api_key)
                    .password(true)
                    .hint_text("Token ..."),
            );

            ui.add_space(8.0);
            ui.small("Stored locally in the workspace root for this demo build.");
            ui.small(format!("Stream status: {}", model.factal_stream_status));

            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button("Save Key").clicked() {
                    save_requested = true;
                }

                if ui.button("Poll Now").clicked() {
                    poll_requested = true;
                }

                if ui.button("Clear Key").clicked() {
                    clear_requested = true;
                }
            });
        });

    if save_requested {
        let trimmed = model.factal_api_key.trim().to_owned();
        match settings_store::save_factal_api_key(&trimmed) {
            Ok(()) => {
                model.factal_api_key = trimmed;
                model.factal_stream_status = if model.has_factal_api_key() {
                    "configured".into()
                } else {
                    "demo".into()
                };
                if model.has_factal_api_key() {
                    model.push_log(
                        "Factal API key saved locally; live polling will start automatically."
                            .into(),
                    );
                    factal_stream::invalidate();
                } else {
                    model.push_log("Factal API key cleared; stream returned to demo mode.".into());
                }
            }
            Err(error) => {
                model.push_log(format!("Factal API key save failed: {}", error));
            }
        }
    }

    if poll_requested {
        if model.has_factal_api_key() {
            factal_stream::invalidate();
            model.factal_stream_status = "syncing".into();
            model.push_log("Factal live poll requested manually.".into());
        } else {
            model.push_log("Factal live poll skipped because no API key is configured.".into());
        }
    }

    if clear_requested {
        match settings_store::save_factal_api_key("") {
            Ok(()) => {
                model.factal_api_key.clear();
                model.factal_stream_status = "demo".into();
                factal_stream::invalidate();
                model.push_log("Factal API key cleared; stream returned to demo mode.".into());
            }
            Err(error) => {
                model.push_log(format!("Factal API key clear failed: {}", error));
            }
        }
    }

    model.factal_settings_open = open;
}
