use crate::model::AppModel;
use crate::theme;

pub fn render_status_log(ctx: &egui::Context, model: &mut AppModel) {
    let collapsed = model.log_collapsed;

    egui::TopBottomPanel::bottom("status_log")
        .resizable(!collapsed)
        .default_height(if collapsed { 32.0 } else { 130.0 })
        .min_height(if collapsed { 32.0 } else { 32.0 })
        .max_height(if collapsed { 32.0 } else { f32::INFINITY })
        .frame(egui::Frame::new().fill(theme::section_background()))
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                // Collapse / expand toggle
                let icon = if collapsed { "▲" } else { "▼" };
                if ui
                    .small_button(icon)
                    .on_hover_text(if collapsed { "Expand log" } else { "Collapse log" })
                    .clicked()
                {
                    model.log_collapsed = !model.log_collapsed;
                }

                ui.heading("Operator Log");

                if let Some(camera) = model.selected_camera() {
                    ui.separator();
                    ui.label(format!("Camera focus: {}", camera.label));
                }

                // When collapsed, show the latest log entry inline
                if collapsed {
                    if let Some(last) = model.activity_log.last() {
                        ui.separator();
                        ui.colored_label(theme::text_muted(), last);
                    }
                }
            });

            if !collapsed {
                ui.add_space(6.0);
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for line in model.activity_log.iter().rev() {
                        ui.label(line);
                    }
                });
            }
        });
}
