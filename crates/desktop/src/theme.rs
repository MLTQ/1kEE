pub fn install(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = egui::vec2(10.0, 10.0);
    style.spacing.button_padding = egui::vec2(12.0, 8.0);
    style.spacing.window_margin = egui::Margin::same(14);
    style.visuals = egui::Visuals::dark();
    style.visuals.panel_fill = egui::Color32::from_rgb(7, 16, 24);
    style.visuals.faint_bg_color = egui::Color32::from_rgb(12, 26, 37);
    style.visuals.extreme_bg_color = egui::Color32::from_rgb(5, 12, 18);
    style.visuals.widgets.active.bg_fill = egui::Color32::from_rgb(31, 91, 110);
    style.visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(22, 64, 78);
    style.visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(11, 25, 35);
    style.visuals.window_fill = egui::Color32::from_rgb(9, 18, 27);
    style.visuals.selection.bg_fill = egui::Color32::from_rgb(22, 120, 146);
    style.visuals.hyperlink_color = egui::Color32::from_rgb(126, 208, 229);
    ctx.set_style(style);
}

pub fn canvas_background() -> egui::Color32 {
    egui::Color32::from_rgb(5, 15, 22)
}

pub fn section_background() -> egui::Color32 {
    egui::Color32::from_rgb(10, 22, 31)
}

pub fn grid_color() -> egui::Color32 {
    egui::Color32::from_rgb(30, 68, 82)
}

pub fn camera_color() -> egui::Color32 {
    egui::Color32::from_rgb(126, 208, 229)
}

pub fn text_muted() -> egui::Color32 {
    egui::Color32::from_rgb(154, 178, 189)
}

pub fn wireframe_color() -> egui::Color32 {
    egui::Color32::from_rgb(66, 123, 143)
}

pub fn topo_color() -> egui::Color32 {
    egui::Color32::from_rgb(39, 88, 105)
}

pub fn hot_color() -> egui::Color32 {
    egui::Color32::from_rgb(245, 125, 78)
}

pub fn contour_color() -> egui::Color32 {
    egui::Color32::from_rgb(96, 164, 181)
}
