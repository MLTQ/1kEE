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
        ui.label("3D terrain · movable light · mineral colors");
        ui.small(
            "Relief refines automatically as you zoom: native seafloor terrain, \
            detailed land elevations, and finer cached surveys where available.",
        );
        ui.small("Contours stay independently selectable and follow the 3D surface. Layer Spread moves terrain and contours together. Colors describe height and depth, not geology.");
        atlas_controls(ui, &mut model.atlas_style);
    }
}

pub(crate) fn atlas_controls(ui: &mut egui::Ui, style: &mut crate::atlas_style::AtlasStyle) {
    use crate::atlas_style::{AtlasStyle, LightMode};
    ui.add_space(10.0);
    ui.strong("Terrain lighting");
    ui.horizontal(|ui| {
        ui.selectable_value(&mut style.light_mode, LightMode::Studio, "Atlas light");
        ui.selectable_value(&mut style.light_mode, LightMode::Sun, "Sunlight");
    });
    match style.light_mode {
        LightMode::Studio => {
            ui.add(
                egui::Slider::new(&mut style.azimuth_deg, 0.0..=360.0)
                    .text("Direction")
                    .suffix("°"),
            );
            ui.small("0° north · 90° east · 180° south · 270° west");
            ui.add(
                egui::Slider::new(&mut style.altitude_deg, 5.0..=85.0)
                    .text("Light height")
                    .suffix("°"),
            );
        }
        LightMode::Sun => {
            ui.small("Follows the app’s date and time. Preview another hour with the offset.");
            ui.add(
                egui::Slider::new(&mut style.time_offset_hours, -12.0..=12.0)
                    .text("Time offset")
                    .suffix(" h"),
            );
        }
    }
    ui.add(egui::Slider::new(&mut style.relief_strength, 0.0..=2.0).text("Relief contrast"));
    ui.add(egui::Slider::new(&mut style.color_richness, 0.0..=1.8).text("Color richness"));
    if ui.button("Reset Atlas appearance").clicked() {
        *style = AtlasStyle::default();
    }
    *style = style.normalized();
    ui.small("Changes appear immediately. Save settings to keep them after restarting.");
}
