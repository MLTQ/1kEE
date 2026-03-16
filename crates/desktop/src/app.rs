use crate::model::AppModel;
use crate::panels;
use crate::theme;

pub struct DashboardApp {
    model: AppModel,
}

impl DashboardApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::install(&cc.egui_ctx);

        Self {
            model: AppModel::seed_demo(),
        }
    }
}

impl eframe::App for DashboardApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        panels::render_header(ctx, &mut self.model);
        panels::render_status_log(ctx, &self.model);
        panels::render_event_list(ctx, &mut self.model);
        panels::render_camera_list(ctx, &mut self.model);

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::canvas_background()))
            .show(ctx, |ui| {
                panels::render_world_map(ui, &mut self.model);
            });
    }
}
