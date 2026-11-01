use crate::model::CameraFeed;
use crossbeam_channel::{Receiver, Sender};
use image::imageops::FilterType;
use reqwest::blocking::{Client, Response};
use reqwest::header::{ACCEPT, CACHE_CONTROL};
use std::io::Read;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
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
    stream_url: String,
    prefer_mjpeg: bool,
    receiver: Receiver<FeedMessage>,
    stop: Arc<AtomicBool>,
}

/// Non-blocking HTTP snapshot/MJPEG playback shared by floating and sidebar feeds.
pub struct CameraFeedPlayer {
    active: Option<ActiveFeed>,
    max_texture_edge: u32,
    pub(crate) texture: Option<egui::TextureHandle>,
    pub(crate) status: String,
    pub(crate) error: Option<String>,
    pub(crate) last_frame_at: Option<Instant>,
    pub(crate) frame_size: Option<[usize; 2]>,
}

impl Default for CameraFeedPlayer {
    fn default() -> Self {
        Self::new(MAX_TEXTURE_EDGE)
    }
}

impl CameraFeedPlayer {
    pub(crate) fn new(max_texture_edge: u32) -> Self {
        Self {
            active: None,
            max_texture_edge: max_texture_edge.clamp(1, MAX_TEXTURE_EDGE),
            texture: None,
            status: String::new(),
            error: None,
            last_frame_at: None,
            frame_size: None,
        }
    }

    pub(crate) fn sync(&mut self, ctx: &egui::Context, camera: &CameraFeed) {
        let prefer_mjpeg = camera.kind.to_ascii_lowercase().contains("mjpeg");
        let matches = self.active.as_ref().is_some_and(|feed| {
            feed.camera_id == camera.id
                && feed.stream_url == camera.stream_url
                && feed.prefer_mjpeg == prefer_mjpeg
        });
        if !matches {
            self.start(ctx, camera);
        }
        self.drain_messages(ctx);
    }

    pub(crate) fn start(&mut self, ctx: &egui::Context, camera: &CameraFeed) {
        self.stop();
        self.status = "Connecting…".into();

        let (sender, receiver) = crossbeam_channel::bounded(3);
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let url = camera.stream_url.clone();
        let prefer_mjpeg = camera.kind.to_ascii_lowercase().contains("mjpeg");
        let repaint = ctx.clone();
        let max_texture_edge = self.max_texture_edge;
        thread::spawn(move || {
            run_feed_worker(
                url,
                prefer_mjpeg,
                sender,
                worker_stop,
                repaint,
                max_texture_edge,
            )
        });

        self.active = Some(ActiveFeed {
            camera_id: camera.id.clone(),
            stream_url: camera.stream_url.clone(),
            prefer_mjpeg,
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

    pub(crate) fn stop(&mut self) {
        if let Some(active) = self.active.take() {
            active.stop.store(true, Ordering::Release);
        }
        self.texture = None;
        self.status.clear();
        self.error = None;
        self.last_frame_at = None;
        self.frame_size = None;
    }

    /// Inert channel-backed playback for UI tests; never starts network work.
    #[cfg(test)]
    pub(crate) fn fixture(
        ctx: &egui::Context,
        camera: &CameraFeed,
        image: egui::ColorImage,
    ) -> (Self, Arc<AtomicBool>) {
        let mut player = Self::new(640);
        let (_, receiver) = crossbeam_channel::bounded(3);
        let stop = Arc::new(AtomicBool::new(false));
        player.active = Some(ActiveFeed {
            camera_id: camera.id.clone(),
            stream_url: camera.stream_url.clone(),
            prefer_mjpeg: camera.kind.to_ascii_lowercase().contains("mjpeg"),
            receiver,
            stop: Arc::clone(&stop),
        });
        player.frame_size = Some(image.size);
        player.texture = Some(ctx.load_texture("camera fixture", image, Default::default()));
        player.status = "Live".into();
        player.last_frame_at = Some(Instant::now());
        (player, stop)
    }
}
impl Drop for CameraFeedPlayer {
    fn drop(&mut self) {
        self.stop();
    }
}

fn run_feed_worker(
    url: String,
    prefer_mjpeg: bool,
    sender: Sender<FeedMessage>,
    stop: Arc<AtomicBool>,
    repaint: egui::Context,
    max_texture_edge: u32,
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
            max_texture_edge,
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
    max_texture_edge: u32,
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
        read_mjpeg_stream(&mut response, sender, stop, repaint, max_texture_edge)
    } else {
        let mut bytes = Vec::new();
        response
            .take((MAX_FRAME_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|error| format!("Could not read camera image: {error}"))?;
        if bytes.len() > MAX_FRAME_BYTES {
            return Err("Camera image exceeded the 12 MB safety limit.".into());
        }
        decode_and_send_frame(&bytes, sender, repaint, max_texture_edge)?;
        interruptible_sleep(stop, SNAPSHOT_INTERVAL);
        Ok(())
    }
}

fn read_mjpeg_stream(
    response: &mut Response,
    sender: &Sender<FeedMessage>,
    stop: &AtomicBool,
    repaint: &egui::Context,
    max_texture_edge: u32,
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
                decode_and_send_frame(&frame, sender, repaint, max_texture_edge)?;
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
    max_texture_edge: u32,
) -> Result<(), String> {
    let mut image = image::load_from_memory(bytes)
        .map_err(|error| format!("Camera response was not a decodable image: {error}"))?;
    let longest = image.width().max(image.height());
    if longest > max_texture_edge {
        let scale = max_texture_edge as f32 / longest as f32;
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

pub(crate) fn fit_size(source: egui::Vec2, available: egui::Vec2) -> egui::Vec2 {
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

    #[test]
    fn shared_player_keeps_matching_feed_and_cancels_changed_endpoints() {
        let ctx = egui::Context::default();
        let mut camera = CameraFeed {
            id: "fixture".into(),
            label: "Fixture".into(),
            provider: "Test".into(),
            kind: "snapshot".into(),
            location: crate::model::GeoPoint { lat: 0.0, lon: 0.0 },
            stream_url: "https://camera.test/frame.jpg".into(),
            last_seen: String::new(),
            status: crate::model::CameraConnectionState::Reachable,
        };
        let (mut player, stop) = CameraFeedPlayer::fixture(
            &ctx,
            &camera,
            egui::ColorImage::new([2, 2], egui::Color32::WHITE),
        );
        let texture = player.texture.as_ref().unwrap().id();
        player.sync(&ctx, &camera);
        assert!(!stop.load(Ordering::Acquire));
        assert_eq!(player.texture.as_ref().unwrap().id(), texture);
        camera.stream_url = "invalid endpoint".into();
        player.sync(&ctx, &camera);
        assert!(stop.load(Ordering::Acquire));
        assert!(
            player.texture.is_none(),
            "old endpoint image must be released"
        );
        assert!(player.last_frame_at.is_none());
        player.stop();
        assert!(player.active.is_none());
        assert!(player.status.is_empty());
    }

    #[test]
    fn sidebar_decoding_bounds_preview_size_and_survives_a_full_queue() {
        let image = image::RgbaImage::from_pixel(1280, 720, image::Rgba([30, 50, 70, 255]));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        let (sender, receiver) = crossbeam_channel::bounded(1);
        let ctx = egui::Context::default();
        decode_and_send_frame(bytes.get_ref(), &sender, &ctx, 640).unwrap();
        // A full channel must drop the next frame instead of blocking decoding.
        decode_and_send_frame(bytes.get_ref(), &sender, &ctx, 640).unwrap();
        match receiver.try_recv().unwrap() {
            FeedMessage::Frame {
                width,
                height,
                rgba,
            } => {
                assert_eq!([width, height], [640, 360]);
                assert_eq!(rgba.len(), 640 * 360 * 4);
            }
            _ => panic!("decoded frame expected"),
        }
    }
}
