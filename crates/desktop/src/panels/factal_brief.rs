use crate::model::AppModel;
use crate::theme;

pub fn render_factal_brief(ctx: &egui::Context, model: &mut AppModel) {
    if !model.factal_brief_open {
        return;
    }

    let Some(event) = model.selected_event().cloned() else {
        model.factal_brief_open = false;
        return;
    };
    let Some(brief) = event.factal_brief.clone() else {
        model.factal_brief_open = false;
        return;
    };

    egui::Window::new("Factal Brief")
        .open(&mut model.factal_brief_open)
        .default_width(520.0)
        .default_height(560.0)
        .frame(egui::Frame::window(&ctx.style()))
        .show(ctx, |ui| {
            ui.heading(event.title.as_str());
            ui.colored_label(event.severity.color(), event.severity.label());
            ui.label(event.location_name.as_str());
            ui.small(format!("Occurred: {}", event.occurred_at));
            ui.small(format!("Source: {}", event.source));

            ui.add_space(10.0);
            ui.colored_label(theme::text_muted(), "Summary");
            ui.label(event.summary.as_str());

            ui.add_space(10.0);
            egui::Grid::new("factal_brief_fields")
                .num_columns(2)
                .spacing([10.0, 6.0])
                .show(ui, |ui| {
                    ui.colored_label(theme::text_muted(), "Factal id");
                    ui.label(brief.factal_id.as_str());
                    ui.end_row();

                    if let Some(severity_value) = brief.severity_value {
                        ui.colored_label(theme::text_muted(), "Severity value");
                        ui.label(severity_value.to_string());
                        ui.end_row();
                    }

                    if let Some(vertical) = &brief.vertical {
                        ui.colored_label(theme::text_muted(), "Vertical");
                        ui.label(vertical.as_str());
                        ui.end_row();
                    }

                    if let Some(subvertical) = &brief.subvertical {
                        ui.colored_label(theme::text_muted(), "Subvertical");
                        ui.label(subvertical.as_str());
                        ui.end_row();
                    }

                    if let Some(point_wkt) = &brief.point_wkt {
                        ui.colored_label(theme::text_muted(), "Point");
                        ui.label(point_wkt.as_str());
                        ui.end_row();
                    }

                    if let Some(raw_date) = &brief.occurred_at_raw {
                        ui.colored_label(theme::text_muted(), "Raw timestamp");
                        ui.label(raw_date.as_str());
                        ui.end_row();
                    }
                });

            if let Some(content) = &brief.content {
                ui.add_space(10.0);
                ui.colored_label(theme::text_muted(), "Factal content");
                ui.label(content.as_str());
            }

            if !brief.topics.is_empty() {
                ui.add_space(10.0);
                ui.colored_label(theme::text_muted(), "Topics");
                ui.horizontal_wrapped(|ui| {
                    for topic in &brief.topics {
                        ui.label(format!("• {}", topic));
                    }
                });
            }

            ui.add_space(10.0);
            ui.collapsing("Raw Factal payload", |ui| {
                egui::ScrollArea::vertical()
                    .max_height(260.0)
                    .show(ui, |ui| {
                        let mut raw_json = brief.raw_json_pretty.clone();
                        ui.add(
                            egui::TextEdit::multiline(&mut raw_json)
                                .font(egui::TextStyle::Monospace)
                                .desired_width(f32::INFINITY)
                                .interactive(false),
                        );
                    });
            });
        });
}
