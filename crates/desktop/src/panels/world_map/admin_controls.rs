use crate::model::{ActiveBody, AppModel, admin::LEVELS};

pub(crate) fn controls(ui: &mut egui::Ui, model: &mut AppModel) {
    if !model.show_admin {
        return;
    }
    ui.indent("admin-options", |ui| {
        ui.small("Boundary levels").on_hover_text("OSM level numbers are shown below. Division names and level assignments vary by country.");
        for (index, (level, label)) in LEVELS.iter().enumerate() {
            ui.checkbox(&mut model.admin_options.enabled[index], format!("{label} ({level})"));
        }
        ui.checkbox(&mut model.admin_options.shade, "Shade regions")
            .on_hover_text("Shade complete regions, preserving holes and islands. Older line-only caches need rebuilding for shading.");
        if model.admin_options.shade {
            ui.label("Opacity");
            ui.add(egui::Slider::new(&mut model.admin_options.opacity, 0.03..=0.5)
                .custom_formatter(|v, _| format!("{:.0}%", v*100.0)));
        }
        if model.active_body != ActiveBody::Earth || !model.globe_view.local_mode {
            ui.small("Boundaries display in Local Earth view.");
        }
        if ui.small_button("Reload boundaries").clicked() { super::admin_layer::reload(); }
        let levels = model.admin_options.levels();
        if levels.is_empty() { ui.small("Select a boundary level."); return; }
        let Some(root) = model.selected_root.as_deref() else { ui.small("Choose a data folder in the terrain library."); return; };
        let Some(data) = super::admin_layer::get(root, &levels) else { ui.small("Loading boundaries…"); return; };
        if data.regions.is_empty() {
            ui.small("No boundaries available. Build Admin Boundaries in the cache builder, then Reload.");
        } else {
            let fill_count = data.regions.iter().filter(|r| r.fill.is_some()).count();
            ui.small(format!("{} regions · {fill_count} shadeable", data.regions.len()));
            if model.admin_options.shade && fill_count < data.regions.len() {
                ui.small("Incomplete / older caches show outlines only. Rebuild to add complete regions.");
            }
        }
        if !data.missing.is_empty() {
            ui.small(format!("Missing levels: {}", data.missing.iter().map(u8::to_string).collect::<Vec<_>>().join(", ")));
        }
        if let Some(warning) = data.warnings.first() { ui.small(warning); }
    });
}
