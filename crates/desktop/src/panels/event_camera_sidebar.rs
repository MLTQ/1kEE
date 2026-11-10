//! A bounded column of live feeds that follows the current event tour stop.
use crate::camera_feed_player::{CameraFeedPlayer, fit_size};
use crate::model::{AppModel, CameraConnectionState, CameraFeed, GeoPoint, haversine_km};
use crate::theme;
use std::collections::HashMap;
use std::time::Duration;

const MAX_FEEDS: usize = 8;
const PREVIEW_EDGE: u32 = 640;

struct RankedCamera {
    camera: CameraFeed,
    distance_km: f32,
}

#[derive(Default)]
pub struct EventCameraSidebar {
    selection_key: Option<(String, GeoPoint, u64)>,
    cameras: Vec<RankedCamera>,
    players: HashMap<String, CameraFeedPlayer>,
}

impl EventCameraSidebar {
    pub fn render(&mut self, ctx: &egui::Context, model: &mut AppModel) {
        if !model.event_follow.enabled() || model.follow_recaps.active.is_some() {
            self.clear();
            return;
        }

        let focus_changed = self.update_selection(model);
        let title = model.event_follow.target().map(|event| event.title.clone());

        egui::SidePanel::right("event_camera_sidebar")
            .default_width(320.0)
            .width_range(280.0..=420.0)
            .resizable(true)
            .frame(
                egui::Frame::side_top_panel(&ctx.style())
                    .inner_margin(8)
                    .fill(theme::panel_fill(255)),
            )
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing = egui::vec2(6.0, 4.0);
                ui.horizontal(|ui| {
                    ui.strong("NEAREST CAMERAS");
                    if title.is_some() {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.small(self.cameras.len().to_string());
                        });
                    }
                });
                if let Some(title) = &title {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(title)
                                .small()
                                .color(theme::text_muted()),
                        )
                        .truncate(),
                    )
                    .on_hover_text(title);
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
                        }
                    });
                    ctx.request_repaint_after(Duration::from_millis(100));
                }
            });
    }

    /// Returns true when the list should scroll back to the closest camera.
    fn update_selection(&mut self, model: &AppModel) -> bool {
        let Some(event) = model.event_follow.target() else {
            self.clear();
            return false;
        };
        let revision = model.camera_registry_revision();
        let mut focus_changed = true;
        if let Some((id, location, previous_revision)) = &self.selection_key {
            focus_changed = *id != event.id || *location != event.location;
            if !focus_changed && revision == *previous_revision {
                return false;
            }
        }
        self.cameras = nearest_feeds(&model.cameras, event.location);
        self.players
            .retain(|id, _| self.cameras.iter().any(|ranked| ranked.camera.id == *id));
        for ranked in &self.cameras {
            self.players
                .entry(ranked.camera.id.clone())
                .or_insert_with(|| CameraFeedPlayer::new(PREVIEW_EDGE));
        }
        self.selection_key = Some((event.id.clone(), event.location, revision));
        focus_changed
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

fn supports_preview(camera:&CameraFeed)->bool {
    preview_source(camera.status,camera.location,&camera.kind,&camera.stream_url)
}

pub(crate) fn nearby_preview(camera:&crate::model::NearbyCamera)->bool {
    preview_source(camera.status,camera.location,&camera.kind,&camera.stream_url)
}

fn preview_source(status:CameraConnectionState,location:GeoPoint,kind:&str,url:&str)->bool {
    if status == CameraConnectionState::Unreachable
        || !location.lat.is_finite()
        || !location.lon.is_finite()
        || location.lat.abs() > 90.0
        || location.lon.abs() > 180.0
    {
        return false;
    }
    let Ok(url) = reqwest::Url::parse(url) else {
        return false;
    };
    let kind = kind.to_ascii_lowercase();
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
        .inner_margin(2)
        .show(ui, |ui| {
            let width = ui.available_width();
            let size = player
                .texture
                .as_ref()
                .map_or(egui::vec2(width, 46.0), |texture| {
                    let aspect = texture.size_vec2().y / texture.size_vec2().x.max(1.0);
                    egui::vec2(width, width * aspect.clamp(9.0 / 16.0, 3.0 / 4.0))
                });
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
            }
            paint_feed_label(ui, rect, ranked, rank);
            let (status, color) = feed_status(player);
            if player.texture.is_none() || player.error.is_some() {
                ui.painter().text(
                    rect.left_bottom() + egui::vec2(5.0, -5.0),
                    egui::Align2::LEFT_BOTTOM,
                    &status,
                    egui::FontId::proportional(10.0),
                    color,
                );
            }
            let mut help = format!(
                "{}\n{} · {}\nClick to open camera feed",
                ranked.camera.label, ranked.camera.provider, status
            );
            if let Some(error) = &player.error {
                help.push_str(&format!("\n{error}"));
            }
            open = response
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text(help)
                .clicked();
        });
    open
}

fn paint_feed_label(ui: &egui::Ui, rect: egui::Rect, ranked: &RankedCamera, rank: usize) {
    let painter = ui.painter().with_clip_rect(rect);
    let band = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), 23.0));
    painter.rect_filled(band, 0, egui::Color32::from_black_alpha(180));
    let distance = painter.layout_no_wrap(
        format!("{:.1} km", ranked.distance_km),
        egui::FontId::proportional(10.0),
        egui::Color32::from_gray(220),
    );
    let distance_pos = egui::pos2(
        band.right() - distance.size().x - 5.0,
        band.center().y - distance.size().y / 2.0,
    );
    let mut job = egui::text::LayoutJob::simple(
        format!("{rank:02}  {}", ranked.camera.label),
        egui::FontId::proportional(12.0),
        egui::Color32::WHITE,
        (distance_pos.x - band.left() - 15.0).max(1.0),
    );
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    let name = ui.fonts(|fonts| fonts.layout_job(job));
    painter.galley(
        egui::pos2(band.left() + 5.0, band.center().y - name.size().y / 2.0),
        name,
        egui::Color32::WHITE,
    );
    painter.galley(distance_pos, distance, egui::Color32::WHITE);
}

fn feed_status(player: &CameraFeedPlayer) -> (String, egui::Color32) {
    if player.error.is_some() {
        ("Retrying…".into(), theme::hot_color())
    } else if let Some(at) = player.last_frame_at {
        let age = at.elapsed().as_secs();
        (
            if age < 5 {
                "Live".to_owned()
            } else {
                format!("Frame {age}s ago")
            },
            theme::camera_color(),
        )
    } else {
        ("Connecting…".into(), theme::text_muted())
    }
}

#[cfg(test)]
#[path = "event_camera_sidebar_tests.rs"]
mod tests;
