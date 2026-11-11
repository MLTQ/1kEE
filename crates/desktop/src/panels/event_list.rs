use crate::model::AppModel;
use crate::theme;

pub fn render_event_list(ctx: &egui::Context, model: &mut AppModel, radio: &mut super::EventRadio) {
    egui::SidePanel::left("event_list")
        .resizable(true)
        .default_width(330.0)
        .frame(egui::Frame::new().fill(theme::section_background()))
        .show(ctx, |ui| {
            ui.heading("Event Queue");
            ui.colored_label(
                theme::text_muted(),
                "Curated incidents that can drive nearby-camera discovery.",
            );
            ui.add_space(8.0);

            let mut focus = None;

            egui::ScrollArea::vertical().show(ui, |ui| {
                for event in &model.events {
                    let is_selected = model.selected_event_id.as_deref() == Some(event.id.as_str());

                    egui::Frame::group(ui.style())
                        .fill(if is_selected {
                            theme::selected_item_fill()
                        } else {
                            theme::item_fill()
                        })
                        .inner_margin(egui::Margin::same(12))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.colored_label(event.severity.color(), event.severity.label());
                                ui.label(event.occurred_at.as_str());
                            });

                            ui.add_space(4.0);
                            ui.strong(event.title.as_str());
                            ui.label(event.location_name.as_str());
                            ui.colored_label(theme::text_muted(), event.summary.as_str());
                            ui.small(format!("Source: {}", event.source));

                            if ui
                                .add_sized(
                                    [ui.available_width(), 28.0],
                                    egui::Button::new(if is_selected {
                                        "Focused"
                                    } else {
                                        "Focus event"
                                    }),
                                )
                                .clicked()
                            {
                                focus = Some(event.id.clone());
                            }
                            if is_selected { radio.button(ui, event); }
                        });

                    ui.add_space(8.0);
                }
            });
            if let Some(id) = focus {
                model.select_event(&id);
            }
        });
}
