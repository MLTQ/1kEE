use crate::model::{AppModel, CameraFeed};
use crate::theme;
use crossbeam_channel::{Receiver, Sender};
use image::imageops::FilterType;
use reqwest::blocking::{Client, Response};
use reqwest::header::{ACCEPT, CACHE_CONTROL};
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(8);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const SNAPSHOT_INTERVAL: Duration = Duration::from_millis(900);
const RECONNECT_INTERVAL: Duration = Duration::from_secs(2);
const MAX_FRAME_BYTES: usize = 12 * 1024 * 1024;
const MAX_TEXTURE_EDGE: u32 = 1_920;

enum FeedMessage {
    Status(String),
    Frame {
        rgba: Vec<u8>,
        width: usize,
        height: usize,
    },
    Error(String),
}

struct ActiveFeed {
    camera_id: String,
    receiver: Receiver<FeedMessage>,
    stop: Arc<AtomicBool>,
}

/// Owns the media worker and texture for the one floating live-camera window.
/// Registry discovery remains independent; the worker starts only after an
/// operator explicitly clicks a camera pip or an Open Feed button.
#[derive(Default)]
pub struct CameraFeedViewer {
    active: Option<ActiveFeed>,
    texture: Option<egui::TextureHandle>,
    status: String,
    error: Option<String>,
    last_frame_at: Option<Instant>,
    frame_size: Option<[usize; 2]>,
}

impl CameraFeedViewer {
    pub fn render(&mut self, ctx: &egui::Context, model: &mut AppModel) {
        if !model.camera_feed_window_open {
            self.stop_active();
            return;
        }

        let Some(camera) = model.selected_camera().cloned() else {
            model.camera_feed_window_open = false;
            self.stop_active();
            return;
        };

        if self.active.as_ref().map(|feed| feed.camera_id.as_str()) != Some(camera.id.as_str()) {
            self.start(ctx, &camera);
        }
        self.drain_messages(ctx);

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
                    ui.strong(&camera.provider);
                    ui.label(&camera.kind);
                    ui.label(format!(
                        "{:.4}°, {:.4}°",
                        camera.location.lat, camera.location.lon
                    ));
                });
                ui.separator();

                if let Some(texture) = &self.texture {
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
                            ui.label(if self.status.is_empty() {
                                "Connecting to feed…"
                            } else {
                                self.status.as_str()
                            });
                        },
                    );
                }

                if let Some(error) = &self.error {
                    ui.colored_label(theme::hot_color(), error);
                } else {
                    let age = self
                        .last_frame_at
                        .map(|at| format!(" · frame {:.1}s ago", at.elapsed().as_secs_f32()))
                        .unwrap_or_default();
                    ui.colored_label(theme::text_muted(), format!("{}{}", self.status, age));
                }

                ui.horizontal(|ui| {
                    if ui.button("Refresh feed").clicked() {
                        refresh_requested = true;
                    }
                    if ui.button("Close").clicked() {
                        close_requested = true;
                    }
                    if let Some([width, height]) = self.frame_size {
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
            self.stop_active();
        } else if refresh_requested {
            self.start(ctx, &camera);
        } else {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    fn start(&mut self, ctx: &egui::Context, camera: &CameraFeed) {
        self.stop_active();
        self.texture = None;
        self.error = None;
        self.last_frame_at = None;
        self.frame_size = None;
        self.status = "Connecting…".into();

        let (sender, receiver) = crossbeam_channel::bounded(3);
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let url = camera.stream_url.clone();
        let prefer_mjpeg = camera.kind.to_ascii_lowercase().contains("mjpeg");
        let repaint = ctx.clone();
        thread::spawn(move || run_feed_worker(url, prefer_mjpeg, sender, worker_stop, repaint));

        self.active = Some(ActiveFeed {
            camera_id: camera.id.clone(),
            receiver,
            stop,
        });
    }

    fn drain_messages(&mut self, ctx: &egui::Context) {
        let Some(active) = &self.active else { return };
        for message in active.receiver.try_iter() {
            match message {
                FeedMessage::Status(status) => {
                    self.status = status;
                }
                FeedMessage::Error(error) => {
                    self.error = Some(error);
                }
                FeedMessage::Frame {
                    rgba,
                    width,
                    height,
                } => {
                    let image = egui::ColorImage::from_rgba_unmultiplied([width, height], &rgba);
                    if let Some(texture) = &mut self.texture {
                        texture.set(image, egui::TextureOptions::LINEAR);
                    } else {
                        self.texture = Some(ctx.load_texture(
                            "camera_feed_frame",
                            image,
                            egui::TextureOptions::LINEAR,
                        ));
                    }
                    self.frame_size = Some([width, height]);
                    self.last_frame_at = Some(Instant::now());
                    self.error = None;
                    self.status = "Live".into();
                }
            }
        }
    }

    fn stop_active(&mut self) {
        if let Some(active) = self.active.take() {
            active.stop.store(true, Ordering::Release);
        }
    }
}

impl Drop for CameraFeedViewer {
    fn drop(&mut self) {
        self.stop_active();
    }
}

fn run_feed_worker(
    url: String,
    prefer_mjpeg: bool,
    sender: Sender<FeedMessage>,
    stop: Arc<AtomicBool>,
    repaint: egui::Context,
) {
    let parsed = match reqwest::Url::parse(&url) {
        Ok(url) if matches!(url.scheme(), "http" | "https") => url,
        Ok(_) => {
            send_message(
                &sender,
                FeedMessage::Error("Only HTTP(S) camera feeds can be opened.".into()),
                &repaint,
            );
            return;
        }
        Err(error) => {
            send_message(
                &sender,
                FeedMessage::Error(format!("Invalid camera feed URL: {error}")),
                &repaint,
            );
            return;
        }
    };
    if !parsed.username().is_empty() || parsed.password().is_some() {
        send_message(
            &sender,
            FeedMessage::Error("Credential-bearing camera URLs are not opened.".into()),
            &repaint,
        );
        return;
    }

    let client = match Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .user_agent("1kEE/0.1 public-camera-viewer")
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            send_message(
                &sender,
                FeedMessage::Error(format!("Could not create feed client: {error}")),
                &repaint,
            );
            return;
        }
    };

    while !stop.load(Ordering::Acquire) {
        send_message(&sender, FeedMessage::Status("Connecting…".into()), &repaint);
        let request_url = materialize_counter_url(parsed.as_str());
        if let Err(error) = read_feed_connection(
            &client,
            &request_url,
            prefer_mjpeg,
            &sender,
            &stop,
            &repaint,
        ) {
            send_message(&sender, FeedMessage::Error(error), &repaint);
            interruptible_sleep(&stop, RECONNECT_INTERVAL);
        }
    }
}

fn read_feed_connection(
    client: &Client,
    url: &str,
    prefer_mjpeg: bool,
    sender: &Sender<FeedMessage>,
    stop: &AtomicBool,
    repaint: &egui::Context,
) -> Result<(), String> {
    let mut response = client
        .get(url)
        .header(
            ACCEPT,
            "multipart/x-mixed-replace,image/jpeg,image/png,image/*;q=0.8",
        )
        .header(CACHE_CONTROL, "no-cache")
        .send()
        .map_err(|error| format!("Feed request failed: {error}"))?;
    if !response.status().is_success() {
        return Err(format!("Feed returned HTTP {}.", response.status()));
    }

    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if prefer_mjpeg
        || content_type.contains("multipart")
        || content_type.contains("x-mixed-replace")
    {
        read_mjpeg_stream(&mut response, sender, stop, repaint)
    } else {
        let mut bytes = Vec::new();
        response
            .take((MAX_FRAME_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|error| format!("Could not read camera image: {error}"))?;
        if bytes.len() > MAX_FRAME_BYTES {
            return Err("Camera image exceeded the 12 MB safety limit.".into());
        }
        decode_and_send_frame(&bytes, sender, repaint)?;
        interruptible_sleep(stop, SNAPSHOT_INTERVAL);
        Ok(())
    }
}

fn read_mjpeg_stream(
    response: &mut Response,
    sender: &Sender<FeedMessage>,
    stop: &AtomicBool,
    repaint: &egui::Context,
) -> Result<(), String> {
    let mut chunk = [0_u8; 16 * 1024];
    let mut buffer = Vec::with_capacity(256 * 1024);
    while !stop.load(Ordering::Acquire) {
        let read = response
            .read(&mut chunk)
            .map_err(|error| format!("MJPEG stream read failed: {error}"))?;
        if read == 0 {
            return Err("MJPEG stream ended; reconnecting…".into());
        }
        buffer.extend_from_slice(&chunk[..read]);
        while let Some(frame) = take_next_jpeg(&mut buffer) {
            if frame.len() <= MAX_FRAME_BYTES {
                decode_and_send_frame(&frame, sender, repaint)?;
            }
        }
        if buffer.len() > MAX_FRAME_BYTES {
            buffer.clear();
            return Err("MJPEG frame exceeded the 12 MB safety limit.".into());
        }
    }
    Ok(())
}

fn decode_and_send_frame(
    bytes: &[u8],
    sender: &Sender<FeedMessage>,
    repaint: &egui::Context,
) -> Result<(), String> {
    let mut image = image::load_from_memory(bytes)
        .map_err(|error| format!("Camera response was not a decodable image: {error}"))?;
    let longest = image.width().max(image.height());
    if longest > MAX_TEXTURE_EDGE {
        let scale = MAX_TEXTURE_EDGE as f32 / longest as f32;
        let width = (image.width() as f32 * scale).round().max(1.0) as u32;
        let height = (image.height() as f32 * scale).round().max(1.0) as u32;
        image = image.resize(width, height, FilterType::Triangle);
    }
    let rgba = image.into_rgba8();
    let (width, height) = rgba.dimensions();
    send_message(
        sender,
        FeedMessage::Frame {
            rgba: rgba.into_raw(),
            width: width as usize,
            height: height as usize,
        },
        repaint,
    );
    Ok(())
}

fn take_next_jpeg(buffer: &mut Vec<u8>) -> Option<Vec<u8>> {
    let start = buffer.windows(2).position(|pair| pair == [0xff, 0xd8])?;
    if start > 0 {
        buffer.drain(..start);
    }
    let end = buffer
        .get(2..)?
        .windows(2)
        .position(|pair| pair == [0xff, 0xd9])?
        + 2;
    let frame_end = end + 2;
    let frame = buffer[..frame_end].to_vec();
    buffer.drain(..frame_end);
    Some(frame)
}

fn send_message(sender: &Sender<FeedMessage>, message: FeedMessage, repaint: &egui::Context) {
    let _ = sender.try_send(message);
    repaint.request_repaint();
}

fn interruptible_sleep(stop: &AtomicBool, duration: Duration) {
    let until = Instant::now() + duration;
    while !stop.load(Ordering::Acquire) && Instant::now() < until {
        thread::sleep(Duration::from_millis(50));
    }
}

fn fit_size(source: egui::Vec2, available: egui::Vec2) -> egui::Vec2 {
    if source.x <= 0.0 || source.y <= 0.0 {
        return available;
    }
    let scale = (available.x / source.x)
        .min(available.y / source.y)
        .min(1.0);
    source * scale
}

fn materialize_counter_url(url: &str) -> String {
    let counter = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    url.replace("COUNTER", &counter.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jpeg_extractor_skips_multipart_headers_and_keeps_trailing_bytes() {
        let mut bytes = b"--frame\r\nContent-Type: image/jpeg\r\n\r\n".to_vec();
        bytes.extend_from_slice(&[0xff, 0xd8, 1, 2, 3, 0xff, 0xd9]);
        bytes.extend_from_slice(b"--next");

        assert_eq!(
            take_next_jpeg(&mut bytes),
            Some(vec![0xff, 0xd8, 1, 2, 3, 0xff, 0xd9])
        );
        assert_eq!(bytes, b"--next");
    }

    #[test]
    fn image_fit_preserves_aspect_ratio_and_never_upscales() {
        assert_eq!(
            fit_size(egui::vec2(1_920.0, 1_080.0), egui::vec2(960.0, 700.0)),
            egui::vec2(960.0, 540.0)
        );
        assert_eq!(
            fit_size(egui::vec2(320.0, 240.0), egui::vec2(960.0, 700.0)),
            egui::vec2(320.0, 240.0)
        );
    }

    #[test]
    fn snapshot_counter_placeholder_is_refreshed_per_request() {
        let url = materialize_counter_url("http://8.8.8.8/camera?COUNTER");
        assert!(url.starts_with("http://8.8.8.8/camera?"));
        assert!(!url.contains("COUNTER"));
    }
}
