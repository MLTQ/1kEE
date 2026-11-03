//! Quantity/region controls for the optional gravity anomaly layer.
use super::{CACHE, clear};
use crate::model::gravity::{GravityKind, gravity_maps};
use crate::model::{ActiveBody, AppModel};

pub(crate) fn controls(ui: &mut egui::Ui, model: &mut AppModel) {
    let response = ui
        .add_enabled(
            model.active_body == ActiveBody::Earth,
            egui::Checkbox::new(&mut model.gravity.enabled, "Gravity anomaly"),
        )
        .on_disabled_hover_text("Available on Earth");
    if response.changed() && !model.gravity.enabled {
        clear();
    }
    if !model.gravity.enabled {
        return;
    }
    let before = model.gravity.selected();
    ui.indent("gravity-anomaly-controls", |ui| {
        let mut kind = model.gravity.map().kind;
        egui::ComboBox::from_id_salt("gravity-quantity")
            .selected_text(kind.label())
            .show_ui(ui, |ui| {
                for option in GravityKind::ALL {
                    ui.selectable_value(&mut kind, option, option.label());
                }
            });
        model.gravity.select_kind(kind);
        let mut selected = model.gravity.selected();
        egui::ComboBox::from_id_salt("gravity-region")
            .selected_text(&model.gravity.map().label)
            .show_ui(ui, |ui| {
                for (index, map) in gravity_maps()
                    .iter()
                    .enumerate()
                    .filter(|(_, map)| map.kind == kind)
                {
                    ui.selectable_value(&mut selected, index, &map.label);
                }
            });
        model.gravity.select_map(selected);
        ui.small(kind.description());
        ui.small(format!("{} grid spacing", model.gravity.map().spacing));
        ui.small(&model.gravity.map().note);
        ui.horizontal(|ui| {
            let range = kind.color_limit();
            for value in [-range, 0.0, range] {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(14.0, 8.0), egui::Sense::hover());
                ui.painter()
                    .rect_filled(rect, 0.0, super::grid::color(value, range));
                ui.small(format!("{value:.0}"));
            }
        });
        ui.small(format!("mGal · colors clip at ±{:.0}", kind.color_limit()));
        if before == model.gravity.selected()
            && let Some(cache) = CACHE.get()
            && let Ok(cache) = cache.lock()
        {
            ui.small(&cache.status);
        }
    });
    if before != model.gravity.selected() {
        clear();
    }
}
