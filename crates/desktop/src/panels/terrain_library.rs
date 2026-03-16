use crate::city_catalog;
use crate::model::AppModel;
use crate::terrain_precompute::{self, PrecomputeJobState};
use crate::theme;

pub fn render_terrain_library(ctx: &egui::Context, model: &mut AppModel) {
    terrain_precompute::tick(model.selected_root.as_deref());

    if terrain_precompute::has_active_jobs(model.selected_root.as_deref()) {
        ctx.request_repaint_after(std::time::Duration::from_millis(350));
    }

    if !model.terrain_library_open {
        return;
    }

    let mut open = model.terrain_library_open;
    egui::Window::new("Terrain Library")
        .open(&mut open)
        .default_size(egui::vec2(560.0, 640.0))
        .min_size(egui::vec2(480.0, 420.0))
        .frame(
            egui::Frame::window(&ctx.style())
                .fill(egui::Color32::from_rgb(14, 18, 23))
                .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(43, 49, 58))),
        )
        .show(ctx, |ui| {
            ui.colored_label(
                theme::text_muted(),
                "Search cities, focus terrain, and queue background contour precompute.",
            );
            ui.add_space(8.0);

            ui.horizontal(|ui| {
                ui.label("City Search");
                ui.add_sized(
                    [ui.available_width(), 28.0],
                    egui::TextEdit::singleline(&mut model.city_filter)
                        .hint_text("Type a city or country..."),
                );
            });

            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if let Some(city) = model.focused_city() {
                    ui.label(format!("Manual focus: {}", city.location_label()));
                    if ui.button("Use Event Focus").clicked() {
                        model.clear_city_focus();
                    }
                } else {
                    ui.colored_label(theme::text_muted(), "Manual focus: event-driven");
                }

                ui.separator();
                ui.label(format!("Selected {}", model.selected_city_ids.len()));
            });

            ui.add_space(8.0);
            ui.separator();
            ui.add_space(8.0);

            egui::ScrollArea::vertical()
                .max_height(280.0)
                .show(ui, |ui| {
                    for city in city_catalog::search(&model.city_filter, 80) {
                        let mut checked = model.selected_city_ids.contains(city.id.as_str());
                        egui::Frame::group(ui.style())
                            .fill(egui::Color32::from_rgb(15, 22, 28))
                            .inner_margin(egui::Margin::symmetric(10, 8))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    if ui.checkbox(&mut checked, "").changed() {
                                        if checked {
                                            model.selected_city_ids.insert(city.id.clone());
                                        } else {
                                            model.selected_city_ids.remove(city.id.as_str());
                                        }
                                    }

                                    ui.vertical(|ui| {
                                        ui.strong(city.location_label());
                                        ui.small(format!(
                                            "{:.4}, {:.4}  ·  pop {:>8}",
                                            city.location.lat, city.location.lon, city.population
                                        ));
                                    });

                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if ui.button("Focus").clicked() {
                                                model.focus_city(city.id.as_str());
                                            }
                                        },
                                    );
                                });
                            });
                        ui.add_space(6.0);
                    }
                });

            ui.horizontal(|ui| {
                if ui.button("Queue Selected").clicked() {
                    let selected: Vec<_> = model.selected_city_ids.iter().cloned().collect();
                    for city_id in &selected {
                        if let Some(city) = city_catalog::by_id(city_id) {
                            terrain_precompute::queue_city(model.selected_root.as_deref(), &city);
                        }
                    }
                    if !selected.is_empty() {
                        model.push_log(format!(
                            "Queued terrain precompute for {} city selection(s).",
                            selected.len()
                        ));
                    }
                }

                if ui.button("Clear Selected").clicked() {
                    model.selected_city_ids.clear();
                }
            });

            ui.add_space(12.0);
            ui.separator();
            ui.add_space(8.0);
            ui.heading("Downloads");

            let snapshots = terrain_precompute::snapshots(model.selected_root.as_deref());
            if snapshots.is_empty() {
                ui.colored_label(theme::text_muted(), "No precompute jobs queued yet.");
            } else {
                let (ongoing, completed): (Vec<_>, Vec<_>) = snapshots
                    .into_iter()
                    .partition(|job| job.state != PrecomputeJobState::Completed);

                if !ongoing.is_empty() {
                    ui.label("Ongoing");
                    for job in ongoing {
                        draw_job_row(ui, &job);
                    }
                }

                if !completed.is_empty() {
                    ui.add_space(8.0);
                    ui.label("Completed");
                    for job in completed {
                        ui.horizontal(|ui| {
                            ui.strong(job.city_label);
                            ui.colored_label(theme::text_muted(), "Ready");
                        });
                    }
                }
            }
        });

    model.terrain_library_open = open;
}

fn draw_job_row(ui: &mut egui::Ui, job: &terrain_precompute::PrecomputeJobSnapshot) {
    let progress = if job.total_assets == 0 {
        0.0
    } else {
        (job.ready_assets as f32 / job.total_assets as f32).clamp(0.0, 1.0)
    };

    egui::Frame::group(ui.style())
        .fill(egui::Color32::from_rgb(15, 22, 28))
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.strong(job.city_label.as_str());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.colored_label(
                        theme::text_muted(),
                        match job.state {
                            PrecomputeJobState::Queued => "queued",
                            PrecomputeJobState::Running => "running",
                            PrecomputeJobState::Completed => "done",
                        },
                    );
                });
            });

            ui.add(
                egui::ProgressBar::new(progress)
                    .desired_width(ui.available_width())
                    .show_percentage(),
            );
            ui.small(format!(
                "{} / {} buckets ready · {} pending",
                job.ready_assets, job.total_assets, job.pending_assets
            ));
        });
    ui.add_space(6.0);
}
