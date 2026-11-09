use crate::{model::AppModel, theme};

pub(super) fn render(ui: &mut egui::Ui, model: &mut AppModel) {
    ui.heading("Map Theme");
    ui.colored_label(
        theme::text_muted(),
        "Choose a map palette. The active theme applies immediately.",
    );
    ui.add_space(8.0);
    ui.horizontal_wrapped(|ui| {
        for &t in theme::MapTheme::ALL {
            let is_active = model.map_theme == t;
            let fill = if is_active {
                theme::chrome_active_fill()
            } else {
                egui::Color32::TRANSPARENT
            };
            let label_color = if is_active {
                theme::chrome_active_text()
            } else {
                theme::text_muted()
            };
            let btn = egui::Button::new(egui::RichText::new(t.label()).color(label_color).small())
                .fill(fill)
                .corner_radius(4.0);
            if ui.add(btn).clicked() {
                model.map_theme = t;
            }
        }
    });
    if let Some(active) = theme::MapTheme::ALL.iter().find(|&&t| t == model.map_theme) {
        ui.small(active.theory());
    }
    if model.map_theme == theme::MapTheme::Atlas {
        ui.add_space(8.0);
        ui.label("3D terrain · measured elevation · northwest light · mineral colors");
        ui.small(
            "Relief refines automatically as you zoom: native seafloor terrain, \
            detailed land elevations, and finer cached surveys where available.",
        );
        ui.small("Contours stay independently selectable and follow the 3D surface. Layer Spread moves terrain and contours together. Colors describe height and depth, not geology.");
    }
}
