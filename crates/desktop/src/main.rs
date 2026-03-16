mod app;
mod model;
mod panels;
mod terrain_assets;
mod theme;

use app::DashboardApp;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1520.0, 920.0])
            .with_min_inner_size([1100.0, 720.0])
            .with_title("1kEE | One Thousand Electric Eye"),
        ..Default::default()
    };

    eframe::run_native(
        "1kEE",
        options,
        Box::new(|cc| Ok(Box::new(DashboardApp::new(cc)))),
    )
}
