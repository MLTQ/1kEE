use crate::camera_feed_player::{CameraFeedPlayer, fit_size};
use crate::model::AppModel;
use crate::theme;
use std::time::Duration;

/// Owns the floating window; playback is shared with the event-follow sidebar.
#[derive(Default)]
pub struct CameraFeedViewer {
    player: CameraFeedPlayer,
}

impl CameraFeedViewer {
    pub fn render(&mut self, ctx: &egui::Context, model: &mut AppModel) {
        if !model.camera_feed_window_open {
            self.player.stop();
            return;
        }

        let Some(camera) = model.selected_camera().cloned() else {
            model.camera_feed_window_open = false;
            self.player.stop();
            return;
        };

        self.player.sync(ctx, &camera);

        let mut open = true;
        let mut refresh_requested = false;
        let mut close_requested = false;
        let title = format!("Live Camera · {}", camera.label);
        egui::Window::new(title)
            .id("camera_feed_viewer".into())
            .open(&mut open)
            .resizable(true)
            .default_size([760.0, 590.0])
            .min_size([420.0, 340.0])
            .frame(
                egui::Frame::window(&ctx.style())
                    .stroke(egui::Stroke::new(1.5, theme::camera_color())),
            )
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.colored_label(camera.status.color(), camera.status.label());
                    ui.strong(
                        camera
                            .provider
                            .strip_prefix("Project Eyes On · ")
                            .unwrap_or(&camera.provider),
                    );
                    ui.label(&camera.kind);
                    ui.label(format!(
                        "{:.4}°, {:.4}°",
                        camera.location.lat, camera.location.lon
                    ));
                });
                ui.separator();

                if let Some(texture) = &self.player.texture {
                    let available = egui::vec2(
                        ui.available_width().max(1.0),
                        (ui.available_height() - 112.0).max(180.0),
                    );
                    let size = fit_size(texture.size_vec2(), available);
                    ui.allocate_ui_with_layout(
                        available,
                        egui::Layout::centered_and_justified(egui::Direction::TopDown),
                        |ui| {
                            ui.add(egui::Image::new((texture.id(), size)));
                        },
                    );
                } else {
                    ui.allocate_ui_with_layout(
                        egui::vec2(ui.available_width(), 240.0),
                        egui::Layout::top_down(egui::Align::Center),
                        |ui| {
                            ui.add_space(70.0);
                            ui.spinner();
                            ui.label(if self.player.status.is_empty() {
                                "Connecting to feed…"
                            } else {
                                self.player.status.as_str()
                            });
                        },
                    );
                }

                if let Some(error) = &self.player.error {
                    ui.colored_label(theme::hot_color(), error);
                } else {
                    let age = self
                        .player
                        .last_frame_at
                        .map(|at| format!(" · frame {:.1}s ago", at.elapsed().as_secs_f32()))
                        .unwrap_or_default();
                    ui.colored_label(
                        theme::text_muted(),
                        format!("{}{}", self.player.status, age),
                    );
                }

                ui.horizontal(|ui| {
                    if ui.button("Refresh feed").clicked() {
                        refresh_requested = true;
                    }
                    if ui.button("Close").clicked() {
                        close_requested = true;
                    }
                    if let Some([width, height]) = self.player.frame_size {
                        ui.small(format!("{width} × {height}"));
                    }
                });
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(&camera.stream_url)
                            .monospace()
                            .small()
                            .color(theme::text_muted()),
                    )
                    .wrap(),
                );
            });

        if !open || close_requested {
            model.camera_feed_window_open = false;
            self.player.stop();
        } else if refresh_requested {
            self.player.start(ctx, &camera);
        } else {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }
}
