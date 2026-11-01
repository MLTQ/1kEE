//! A bounded column of live feeds that follows the current event tour stop.
use crate::camera_feed_player::{CameraFeedPlayer, fit_size};
use crate::model::{AppModel, CameraConnectionState, CameraFeed, GeoPoint, haversine_km};
use crate::theme;
use std::collections::HashMap;
use std::time::Duration;

const MAX_FEEDS: usize = 4;
const PREVIEW_EDGE: u32 = 640;

struct RankedCamera {
    camera: CameraFeed,
    distance_km: f32,
}

#[derive(Default)]
pub struct EventCameraSidebar {
    selection_key: Option<(u32, u32, u64)>,
    cameras: Vec<RankedCamera>,
    players: HashMap<String, CameraFeedPlayer>,
}

impl EventCameraSidebar {
    pub fn render(&mut self, ctx: &egui::Context, model: &mut AppModel) {
        if !model.event_follow.enabled() {
            self.clear();
            return;
        }

        let event = model
            .selected_event()
            .filter(|_| model.event_follow.target().is_some());
        let title = event.map(|event| event.title.clone());
        let location = event.map(|event| event.location_name.clone());
        let previous_focus = self.selection_key.map(|(lat, lon, _)| (lat, lon));
        self.update_selection(model, event.map(|event| event.location));
        let focus_changed = previous_focus != self.selection_key.map(|(lat, lon, _)| (lat, lon));

        egui::SidePanel::right("event_camera_sidebar")
            .default_width(320.0)
            .width_range(280.0..=420.0)
            .resizable(true)
            .frame(
                egui::Frame::side_top_panel(&ctx.style())
                    .inner_margin(12)
                    .fill(theme::panel_fill(255)),
            )
            .show(ctx, |ui| {
                ui.strong("NEAREST CAMERAS");
                if let Some(title) = &title {
                    ui.add(egui::Label::new(title).truncate())
                        .on_hover_text(title);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(location.as_deref().unwrap_or_default())
                                .small()
                                .color(theme::text_muted()),
                        )
                        .truncate(),
                    );
                    ui.small(format!(
                        "{} closest feeds · nearest first",
                        self.cameras.len()
                    ));
                }
                ui.separator();

                if title.is_none() {
                    ui.label("Waiting for a live event to follow.");
                } else if self.cameras.is_empty() {
                    ui.label("No camera feeds available for this event.");
                    ui.colored_label(theme::text_muted(), &model.camera_registry_status);
                    if !model.obs_view && ui.button("Camera sources…").clicked() {
                        model.settings_open = true;
                    }
                } else {
                    let mut scroll = egui::ScrollArea::vertical()
                        .id_salt("event_camera_feeds")
                        .auto_shrink([false, false]);
                    if focus_changed {
                        scroll = scroll.vertical_scroll_offset(0.0);
                    }
                    scroll.show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        for (index, ranked) in self.cameras.iter().enumerate() {
                            let player = self.players.get_mut(&ranked.camera.id).unwrap();
                            player.sync(ctx, &ranked.camera);
                            ui.push_id(&ranked.camera.id, |ui| {
                                if render_feed(ui, ranked, index + 1, player) {
                                    model.open_camera_feed(&ranked.camera.id);
                                }
                            });
                            ui.add_space(8.0);
                        }
                    });
                    ctx.request_repaint_after(Duration::from_millis(100));
                }
            });
    }

    fn update_selection(&mut self, model: &AppModel, focus: Option<GeoPoint>) {
        let Some(focus) = focus else {
            self.clear();
            return;
        };
        let key = (
            focus.lat.to_bits(),
            focus.lon.to_bits(),
            model.camera_registry_revision(),
        );
        if self.selection_key == Some(key) {
            return;
        }
        self.cameras = nearest_feeds(&model.cameras, focus);
        self.players
            .retain(|id, _| self.cameras.iter().any(|ranked| ranked.camera.id == *id));
        for ranked in &self.cameras {
            self.players
                .entry(ranked.camera.id.clone())
                .or_insert_with(|| CameraFeedPlayer::new(PREVIEW_EDGE));
        }
        self.selection_key = Some(key);
    }

    fn clear(&mut self) {
        self.players.clear();
        self.cameras.clear();
        self.selection_key = None;
    }
}

fn nearest_feeds(cameras: &[CameraFeed], focus: GeoPoint) -> Vec<RankedCamera> {
    let mut ranked: Vec<_> = cameras
        .iter()
        .filter_map(|camera| {
            let distance = haversine_km(focus, camera.location);
            (distance.is_finite() && supports_preview(camera)).then_some((camera, distance))
        })
        .collect();
    let order = |a: &(&CameraFeed, f32), b: &(&CameraFeed, f32)| {
        a.1.total_cmp(&b.1).then_with(|| a.0.id.cmp(&b.0.id))
    };
    if ranked.len() > MAX_FEEDS {
        ranked.select_nth_unstable_by(MAX_FEEDS, order);
        ranked.truncate(MAX_FEEDS);
    }
    ranked.sort_by(order);
    ranked
        .into_iter()
        .map(|(camera, distance_km)| RankedCamera {
            camera: camera.clone(),
            distance_km,
        })
        .collect()
}

fn supports_preview(camera: &CameraFeed) -> bool {
    if camera.status == CameraConnectionState::Unreachable
        || !camera.location.lat.is_finite()
        || !camera.location.lon.is_finite()
        || camera.location.lat.abs() > 90.0
        || camera.location.lon.abs() > 180.0
    {
        return false;
    }
    let Ok(url) = reqwest::Url::parse(&camera.stream_url) else {
        return false;
    };
    let kind = camera.kind.to_ascii_lowercase();
    let path = url.path().to_ascii_lowercase();
    matches!(url.scheme(), "http" | "https")
        && url.username().is_empty()
        && url.password().is_none()
        && !["hls", "mp4", "html"]
            .iter()
            .any(|format| kind.contains(format))
        && ![".m3u8", ".mp4", ".html", ".htm"]
            .iter()
            .any(|extension| path.ends_with(extension))
}

fn render_feed(
    ui: &mut egui::Ui,
    ranked: &RankedCamera,
    rank: usize,
    player: &CameraFeedPlayer,
) -> bool {
    let mut open = false;
    egui::Frame::new()
        .fill(theme::canvas_background())
        .stroke(egui::Stroke::new(
            1.0,
            theme::camera_color().gamma_multiply(0.35),
        ))
        .corner_radius(5)
        .inner_margin(8)
        .show(ui, |ui| {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(format!("{rank:02}  {}", ranked.camera.label)).strong(),
                )
                .truncate(),
            )
            .on_hover_text(&ranked.camera.label);
            ui.horizontal(|ui| {
                let provider = ranked
                    .camera
                    .provider
                    .strip_prefix("Project Eyes On · ")
                    .unwrap_or(&ranked.camera.provider);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.small(format!("{:.1} km", ranked.distance_km));
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(provider)
                                .small()
                                .color(theme::text_muted()),
                        )
                        .truncate(),
                    );
                });
            });
            let size = egui::vec2(ui.available_width(), ui.available_width() * 9.0 / 16.0);
            let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
            ui.painter().rect_filled(rect, 3, egui::Color32::BLACK);
            if let Some(texture) = &player.texture {
                let image_rect = egui::Rect::from_center_size(
                    rect.center(),
                    fit_size(texture.size_vec2(), size),
                );
                ui.painter().image(
                    texture.id(),
                    image_rect,
                    egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            } else {
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    if player.error.is_some() {
                        "Feed unavailable"
                    } else {
                        "Connecting…"
                    },
                    egui::FontId::proportional(13.0),
                    theme::text_muted(),
                );
            }
            open |= response.on_hover_text("Open camera feed").clicked();
            ui.horizontal(|ui| {
                if let Some(error) = &player.error {
                    ui.colored_label(theme::hot_color(), "Retrying…")
                        .on_hover_text(error);
                } else if let Some(at) = player.last_frame_at {
                    let age = at.elapsed().as_secs();
                    ui.colored_label(
                        theme::camera_color(),
                        if age < 5 {
                            "Live".to_owned()
                        } else {
                            format!("Frame {age}s ago")
                        },
                    );
                } else {
                    ui.spinner();
                    ui.small("Connecting…");
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    open |= ui.small_button("Open feed").clicked();
                });
            });
        });
    open
}

#[cfg(test)]
#[path = "event_camera_sidebar_tests.rs"]
mod tests;
