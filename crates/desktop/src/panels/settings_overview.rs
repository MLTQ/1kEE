//! Operational information formerly spread across the top banner.
use crate::{model::AppModel, osm_ingest, terrain_assets, theme};

pub(super) fn render(ui: &mut egui::Ui, model: &mut AppModel) {
    ui.heading("Overview");
    ui.colored_label(theme::text_muted(), "Live sources and local data");
    ui.add_space(10.0);
    egui::Grid::new("source-status")
        .spacing([24.0, 10.0])
        .show(ui, |ui| {
            for (name, value) in [
                ("Factal stream", model.factal_stream_status.as_str()),
                ("USGS quakes", model.usgs_stream_status.as_str()),
                ("Camera registry", model.camera_registry_status.as_str()),
                ("Terrain", model.terrain_inventory.status_label()),
                ("OSM", model.osm_inventory.status_label()),
            ] {
                ui.colored_label(theme::text_muted(), name);
                ui.label(value);
                ui.end_row();
            }
            ui.colored_label(theme::text_muted(), "Loaded records");
            ui.label(format!(
                "{} events · {} cameras",
                model.events.len(),
                model.cameras.len()
            ));
            ui.end_row();
        });
    ui.add_space(12.0);
    if model.camera_registry_status == "inactive"
        && !model.eyes_on_enabled
        && ui
            .button("Enable live cameras")
            .on_hover_text("Enable the paced public-directory source and refresh now")
            .clicked()
    {
        super::actions::request_camera_scan(model);
    }
    if model.camera_registry_scanning {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.add(
                egui::ProgressBar::new(model.camera_registry_progress.clamp(0.0, 1.0))
                    .desired_width(ui.available_width())
                    .text(if model.camera_registry_progress_label.is_empty() {
                        "Searching for cameras…"
                    } else {
                        &model.camera_registry_progress_label
                    }),
            );
        });
    }
    if let Some(note) = osm_ingest::active_job_note() {
        ui.label(format!("OSM job: {note}"));
    }
    ui.separator();
    ui.label(format!("Focus: {}", model.terrain_focus_location_name()));
    ui.add_space(8.0);
    ui.strong("Runtime sources");
    ui.label(model.terrain_inventory.primary_runtime_source);
    ui.label(model.osm_inventory.primary_runtime_source);
    if let Some(root) = &model.selected_root {
        ui.label(format!("Asset folder: {}", root.display()));
    }
    if let Some(root) = terrain_assets::find_srtm_root(model.selected_root.as_deref()) {
        ui.label(format!("SRTM: {}", root.display()));
    }
    if let Some(path) = osm_ingest::find_planet_pbf(model.selected_root.as_deref()) {
        ui.label(format!("Planet PBF: {}", path.display()));
    }
}
