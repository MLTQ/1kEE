use crate::model::AppModel;
use crate::theme;

pub fn render_status_log(ctx: &egui::Context, model: &AppModel) {
    egui::TopBottomPanel::bottom("status_log")
        .resizable(true)
        .default_height(130.0)
        .frame(egui::Frame::new().fill(theme::section_background()))
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Operator Log");
                if let Some(camera) = model.selected_camera() {
                    ui.separator();
                    ui.label(format!("Camera focus: {}", camera.label));
                }
            });

            ui.add_space(6.0);

            egui::ScrollArea::vertical().show(ui, |ui| {
                for line in model.activity_log.iter().rev() {
                    ui.label(line);
                }
            });
        });
}
