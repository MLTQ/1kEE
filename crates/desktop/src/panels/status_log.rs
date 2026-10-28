use crate::{model::AppModel, theme};

pub(super) fn render(ui: &mut egui::Ui, model: &AppModel) {
    ui.heading("Activity");
    ui.colored_label(
        theme::text_muted(),
        "Recent source, import and cache activity.",
    );
    if let Some(camera) = model.selected_camera() {
        ui.label(format!("Camera focus: {}", camera.label));
    }
    ui.add_space(8.0);
    if model.activity_log.is_empty() {
        ui.small("No recent activity.");
    }
    for line in model.activity_log.iter().rev() {
        ui.label(line);
    }
}
