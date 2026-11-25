//! Controls for consolidating native Earth contour caches.
use super::{ActiveTab, BuilderApp};
use crate::job::{BuildJob, spawn_job_with_wake};
use eframe::egui;
use std::path::PathBuf;
use tile_archive::contour_grid::SRTM_NATIVE_DB_NAME;

fn sources(ui: &mut egui::Ui, paths: &mut Vec<PathBuf>, directories: bool) {
    paths.retain(|path| {
        ui.horizontal_wrapped(|ui| {
            let keep = !ui.small_button("Remove").clicked();
            ui.label(path.display().to_string());
            keep
        })
        .inner
    });
    let label = if directories {
        "Add repair folders…"
    } else {
        "Add old cache files…"
    };
    if ui.button(label).clicked() {
        let picker = rfd::FileDialog::new();
        let selected = if directories {
            picker.pick_folders()
        } else {
            picker.pick_files()
        };
        for path in selected.into_iter().flatten() {
            if !paths.contains(&path) {
                paths.push(path);
            }
        }
    }
}

impl BuilderApp {
    pub(super) fn show_recovery_controls(&mut self, ui: &mut egui::Ui) {
        ui.heading("Recover Earth contours");
        ui.label("Combine native SRTM caches and per-tile repairs into one verified database.");
        ui.add_space(8.0);
        let form = &mut self.recovery_form;
        sources(ui, &mut form.caches, false);
        sources(ui, &mut form.repairs, true);
        ui.separator();
        ui.checkbox(
            &mut form.cleanup_only,
            "Verify an existing output and retire matching inputs",
        );
        if !form.cleanup_only {
            ui.checkbox(
                &mut form.remove_inputs,
                "Retire matching inputs after validation",
            );
        }
        if ui.button("Choose output database…").clicked() {
            let dialog = rfd::FileDialog::new().set_file_name(SRTM_NATIVE_DB_NAME);
            if let Some(path) = if form.cleanup_only {
                dialog.pick_file()
            } else {
                dialog.save_file()
            } {
                form.output = path;
            }
        }
        if !form.output.as_os_str().is_empty() {
            ui.label(form.output.display().to_string());
        }
        ui.collapsing("Validation drive", |ui| {
            ui.label("Use an exact copy on a faster drive for the full database check.");
            if ui.button("Choose verification copy…").clicked() {
                if let Some(path) = rfd::FileDialog::new()
                    .set_file_name("contour-verification.sqlite")
                    .save_file()
                {
                    form.verification_copy = Some(path);
                }
            }
            if let Some(path) = &form.verification_copy {
                ui.label(path.display().to_string());
                if ui.button("Verify on archive drive").clicked() {
                    form.verification_copy = None;
                }
            }
        });
        let ready = self.active_job.is_none()
            && !form.output.as_os_str().is_empty()
            && !(form.caches.is_empty() && form.repairs.is_empty());
        let label = if form.cleanup_only {
            "Verify and retire inputs"
        } else {
            "Build recovered cache"
        };
        if ui.add_enabled(ready, egui::Button::new(label)).clicked() {
            self.active_job = Some(spawn_job_with_wake(
                BuildJob::RecoverContours(form.clone()),
                Some(ui.ctx().clone()),
            ));
            self.active_job_tab = Some(ActiveTab::Recovery);
            self.status = "Checking terrain caches".into();
            self.progress_detail.clear();
            self.needs_fresh_working_folder = false;
            self.log_lines.clear();
        }
    }
    pub(super) fn show_recovery_log(&self, ui: &mut egui::Ui) {
        ui.heading("Recovery progress");
        ui.horizontal(|ui| {
            ui.label(&self.status);
            if self.active_job_tab == Some(ActiveTab::Recovery) {
                ui.label(
                    egui::RichText::new("· Running…")
                        .color(egui::Color32::from_rgb(95, 210, 190)),
                );
            }
        });
        ui.separator();
        let row_height = ui.text_style_height(&egui::TextStyle::Body);
        let count = self.log_lines.len();
        egui::ScrollArea::vertical()
            .id_salt("recovery_log_scroll")
            .show_rows(ui, row_height, count, |ui, range| {
                for row in range {
                    let idx = count - 1 - row;
                    if let Some(line) = self.log_lines.get(idx) {
                        ui.label(line);
                    }
                }
            });
    }
}
