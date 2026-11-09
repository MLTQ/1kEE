//! Atlas-specific controls and live build totals, separate from OSM inventory.
use super::{ActiveTab, BuilderApp};
use crate::{
    atlas,
    job::{BuildJob, spawn_job},
};
use eframe::egui;
use std::path::PathBuf;
use tile_archive::atlas::{FILE_NAME, Tier};
#[derive(Default)]
pub(super) struct Form {
    initialized: bool,
    data: String,
    srtm: String,
    output: String,
    gdal: String,
    world: bool,
    bounds: [String; 4],
    tiers: [bool; 3],
    pub progress: Option<atlas::Progress>,
}
fn path_field(ui: &mut egui::Ui, label: &str, text: &mut String, output: bool) {
    ui.label(label);
    ui.horizontal(|ui| {
        ui.text_edit_singleline(text);
        if ui.small_button("…").clicked() {
            let selected = if output {
                rfd::FileDialog::new().set_file_name(FILE_NAME).save_file()
            } else {
                rfd::FileDialog::new().pick_folder()
            };
            if let Some(path) = selected {
                *text = path.display().to_string();
            }
        }
    });
}
impl Form {
    fn command(&self) -> Result<atlas::Command, String> {
        let bounds = if self.world {
            [-180., -90., 180., 90.]
        } else {
            let mut b = [0.; 4];
            for (i, value) in self.bounds.iter().enumerate() {
                b[i] = value
                    .trim()
                    .parse()
                    .map_err(|_| "Enter valid Atlas bounds")?;
            }
            b
        };
        tile_archive::atlas::validate_bounds(bounds)?;
        let tiers: Vec<_> = Tier::ALL
            .into_iter()
            .zip(self.tiers)
            .filter_map(|(tier, on)| on.then_some(tier))
            .collect();
        if tiers.is_empty() {
            return Err("Select at least one Atlas tier".into());
        }
        if self.data.trim().is_empty() || self.output.trim().is_empty() {
            return Err("Choose the Data folder and Atlas cache output".into());
        }
        let data_root = PathBuf::from(self.data.trim());
        let srtm_root = if !self.srtm.trim().is_empty() {
            Some(PathBuf::from(self.srtm.trim()))
        } else {
            let root = data_root.join("srtm_gl1");
            root.is_dir().then_some(root)
        };
        Ok(atlas::Command {
            data_root,
            srtm_root,
            cache: self.output.trim().into(),
            gdal_bin_dir: self.gdal.trim().into(),
            bounds,
            tiers,
        })
    }
}
impl BuilderApp {
    pub(super) fn show_atlas_panel(&mut self, ui: &mut egui::Ui) {
        if !self.atlas_form.initialized {
            let derived = PathBuf::from(&self.form.cache_dir)
                .parent()
                .unwrap_or(std::path::Path::new("Derived"))
                .to_path_buf();
            self.atlas_form = Form {
                initialized: true,
                data: if self.form.srtm_root.is_empty() {
                    String::new()
                } else {
                    PathBuf::from(&self.form.srtm_root)
                        .parent()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default()
                },
                srtm: self.form.srtm_root.clone(),
                output: derived
                    .join("terrain")
                    .join(FILE_NAME)
                    .display()
                    .to_string(),
                gdal: self.form.gdal_bin_dir.clone(),
                world: true,
                bounds: [
                    self.form.min_lon.clone(),
                    self.form.min_lat.clone(),
                    self.form.max_lon.clone(),
                    self.form.max_lat.clone(),
                ],
                tiers: [true, true, false],
                progress: None,
            };
        }
        ui.heading("Atlas terrain tiles");
        ui.label("Prebuild measured terrain for Globe and Local views. Lighting and colors stay adjustable.");
        ui.separator();
        let form = &mut self.atlas_form;
        ui.add_enabled_ui(self.active_job.is_none(), |ui| {
            path_field(ui, "Data folder (contains GEBCO)", &mut form.data, false);
            path_field(ui, "SRTM folder (optional for land detail)", &mut form.srtm, false);
            path_field(ui, "Atlas cache output", &mut form.output, true);
            path_field(ui, "GDAL bin folder (blank uses PATH)", &mut form.gdal, false);
            ui.separator();
            ui.horizontal(|ui| {
                ui.selectable_value(&mut form.world, true, "Full planet");
                ui.selectable_value(&mut form.world, false, "Bounding box");
            });
            if !form.world {
                egui::Grid::new("atlas_bounds").show(ui, |ui| {
                    for (i, label) in ["West", "South", "East", "North"].iter().enumerate() {
                        ui.label(*label);
                        ui.text_edit_singleline(&mut form.bounds[i]);
                        ui.end_row();
                    }
                });
                ui.small("For a date-line crossing, use an east longitude above 180°.");
            }
            ui.separator();
            for (i, tier) in Tier::ALL.iter().enumerate() {
                ui.checkbox(&mut form.tiers[i], tier.name());
            }
            ui.small("Global overview + terrain: about 15.2 GB of height data, plus database overhead.");
            ui.small("Land detail uses available SRTM coverage and can require hundreds of GB globally. Fine-provider terrain remains available on demand.");
        });
        ui.add_space(10.);
        if ui
            .add_enabled(
                self.active_job.is_none(),
                egui::Button::new("Build / verify / resume Atlas cache"),
            )
            .clicked()
        {
            match self.atlas_form.command() {
                Ok(command) => {
                    self.status = "Starting Atlas build".into();
                    self.progress = 0.;
                    self.progress_detail = "Checking sources and saved tiles…".into();
                    self.atlas_form.progress = None;
                    self.active_job_tab = Some(ActiveTab::Atlas);
                    self.active_job = Some(spawn_job(BuildJob::Atlas(command)));
                }
                Err(error) => {
                    self.status = "Failed".into();
                    self.progress_detail = error;
                }
            }
        }
        ui.small("Choose Derived/terrain/atlas_heights_v1.sqlite under the desktop's Derived folder for automatic discovery. Verified matching tiles are reused; changed sources and unreadable tiles are rebuilt.");
    }
    pub(super) fn show_atlas_dashboard(&self, ui: &mut egui::Ui) {
        ui.heading("Atlas build overview");
        if let Some(p) = &self.atlas_form.progress {
            ui.label(&p.state);
            ui.label(&p.message);
        } else if self.active_job.is_none() {
            ui.label(&self.progress_detail);
        }
        if let Some(p) = &self.atlas_form.progress {
            ui.add(
                egui::ProgressBar::new(p.fraction())
                    .text(format!("{} / {} tiles ready", p.done, p.total)),
            );
            ui.add_space(12.);
            egui::Grid::new("atlas_totals")
                .spacing([35., 12.])
                .show(ui, |ui| {
                    for (label, value) in [
                        ("Built this run", p.built.to_string()),
                        ("Verified and reused", p.reused.to_string()),
                        ("Remaining", p.total.saturating_sub(p.done).to_string()),
                        ("Outside SRTM coverage", p.unavailable.to_string()),
                        (
                            "Tile data written",
                            format!("{:.2} GB", p.bytes as f64 / 1e9),
                        ),
                        (
                            "Elapsed",
                            format!("{}m {}s", p.elapsed as u64 / 60, p.elapsed as u64 % 60),
                        ),
                        (
                            "Verified or built per minute",
                            format!("{:.1} tiles/min", p.done as f64 / p.elapsed.max(1.) * 60.),
                        ),
                    ] {
                        ui.label(label);
                        ui.strong(value);
                        ui.end_row();
                    }
                });
        } else {
            ui.label("Choose coverage and detail tiers, then start a build. Completed tiles survive interruptions; reopening the same cache verifies and resumes them.");
        }
        ui.separator();
        ui.label("Atlas reads these height tiles directly. Missing coverage falls back to original terrain sources; illumination and forest overlays are applied at runtime.");
        egui::CollapsingHeader::new("Build log").show(ui, |ui| {
            for line in self.log_lines.iter().rev().take(80) {
                ui.small(line);
            }
        });
    }
}
