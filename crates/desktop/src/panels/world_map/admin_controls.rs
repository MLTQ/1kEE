use crate::model::{ActiveBody, AppModel, admin::LEVELS};

pub(crate) fn controls(ui: &mut egui::Ui, model: &mut AppModel) {
    if !model.show_admin {
        return;
    }
    ui.indent("admin-options", |ui| {
        ui.label("Boundary opacity");
        opacity_slider(ui, &mut model.admin_options.layer_opacity, 0.0..=100.0)
            .on_hover_text("Fade boundary lines and region shading together in Globe and Local.");
        ui.small("Boundary levels").on_hover_text("OSM level numbers are shown below. Division names and level assignments vary by country.");
        for (index, (level, label)) in LEVELS.iter().enumerate() {
            ui.checkbox(&mut model.admin_options.enabled[index],
                egui::RichText::new(format!("{label} ({level})"))
                    .color(crate::theme::admin_color(*level)));
        }
        ui.checkbox(&mut model.admin_options.shade, "Shade regions")
            .on_hover_text("Shade complete regions, preserving holes and islands. Older line-only caches need rebuilding for shading.");
        if model.admin_options.shade {
            ui.label("Shading opacity");
            opacity_slider(ui, &mut model.admin_options.opacity, 3.0..=50.0);
        }
        if model.active_body != ActiveBody::Earth {
            ui.small("Boundaries are available on Earth in Globe and Local.");
        }
        source_controls(ui, model);
        let dirs = super::admin_source::directories(model);
        if dirs.is_empty() { ui.small("Load a PBF or open a boundary cache to get started."); return; }
        if let Some(status) = super::admin_source::poll(&dirs) {
            if let Some(coverage) = &status.coverage { ui.small(coverage); }
            if status.running { ui.small("Extraction running. Completed boundary files will load automatically."); }
            if let Some(error) = &status.error { ui.small(error); }
        }
        let levels = model.admin_options.levels();
        if levels.is_empty() { ui.small("Select a boundary level."); return; }
        let Some(data) = super::admin_layer::get_for(model) else { ui.small("Loading boundaries…"); return; };
        if data.regions.is_empty() {
            ui.small("No boundaries loaded yet. Load from PBF or open an existing boundary cache.");
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

fn opacity_slider(
    ui: &mut egui::Ui,
    opacity: &mut f32,
    percent_range: std::ops::RangeInclusive<f32>,
) -> egui::Response {
    let mut percent = *opacity * 100.0;
    let response = ui.add(egui::Slider::new(&mut percent, percent_range)
        .step_by(1.0).suffix("%").max_decimals(0));
    if response.changed() {
        *opacity = percent / 100.0;
    }
    response
}

fn source_controls(ui: &mut egui::Ui, model: &mut AppModel) {
    let job = super::admin_source::current();
    let active = job.as_ref().is_some_and(|j| j.active);
    if ui
        .add_enabled(!active, egui::Button::new("Load from PBF…").small())
        .clicked()
    {
        let mut picker = rfd::FileDialog::new().add_filter("OpenStreetMap PBF", &["pbf"]);
        if let Some(root) = model.selected_root.as_deref() {
            picker = picker.set_directory(root);
        }
        if let Some(source) = picker.pick_file() {
            let suggested = super::admin_source::directories(model)
                .into_iter()
                .next()
                .unwrap_or_else(|| {
                    source
                        .parent()
                        .unwrap_or(std::path::Path::new("."))
                        .join("Derived/osm")
                });
            if let Some(output) = rfd::FileDialog::new()
                .set_title("Choose the boundary cache output folder")
                .set_directory(&suggested)
                .pick_folder()
            {
                let output = super::admin_source::cache_root(&output);
                model.admin_options.cache_root = Some(output.clone());
                super::admin_source::start(source, output);
            }
        }
    }
    if ui.small_button("Open boundary cache…").clicked() {
        if let Some(root) = rfd::FileDialog::new()
            .set_title("Choose a folder containing admin_cells (or admin_cells itself)")
            .pick_folder()
        {
            model.admin_options.cache_root = Some(super::admin_source::cache_root(&root));
            super::admin_layer::reload();
        }
    }
    if ui.small_button("Reload boundaries").clicked() {
        super::admin_layer::reload();
    }
    if let Some(root) = &model.admin_options.cache_root {
        ui.small(format!("Cache: {}", root.display()));
        if ui.small_button("Use data folder cache").clicked() {
            model.admin_options.cache_root = None;
            super::admin_layer::reload();
        }
    }
    if let Some(job) = job {
        if job.active {
            ui.add(egui::ProgressBar::new(job.fraction.clamp(0.0, 1.0)).animate(true));
        }
        ui.small(&job.message).on_hover_text(format!(
            "Source: {}\nOutput: {}",
            job.source.display(),
            job.output.display()
        ));
    }
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_secs(3));
}
