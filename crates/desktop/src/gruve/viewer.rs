//! Lightweight native client. No AppModel, local datasets, feed workers or announcement.
use super::{Navigation, surface::MAX_EDGE};
use crossbeam_channel::{Sender, TrySendError};
use reqwest::{Url, blocking::Client};
use std::{
    collections::VecDeque,
    io::{Cursor, Read},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

fn endpoint(input: &str) -> Result<String, String> {
    let input = input.trim();
    let address = if let Ok(port) = input.parse::<u16>() {
        if port == 0 {
            return Err("Enter a control port between 1 and 65535".into());
        }
        format!("http://127.0.0.1:{port}")
    } else if !input.contains('/') && input.contains(':') {
        format!("http://{input}")
    } else if input.starts_with("/peer/") || input.starts_with("/apps/") {
        format!("http://{}{input}", gruve_sdk::agent_addr())
    } else {
        input.to_owned()
    };
    let mut url = Url::parse(&address).map_err(|_| "Enter a control port or a Gruve app URL")?;
    let local = url.host_str() == Some("localhost")
        || url.host_str().is_some_and(|h| {
            h.trim_matches(['[', ']'])
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
        });
    if !local
        || !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err("Use the URL from the 1kEE tile on your local Gruve agent".into());
    }
    url.set_query(None);
    url.set_fragment(None);
    let path = url
        .path()
        .trim_end_matches('/')
        .trim_end_matches("/index.html");
    if path.ends_with("/apps/1kee") {
        let path = format!("{path}/__gruve/api");
        url.set_path(&path);
    } else if !path.is_empty() && !path.ends_with("/apps/1kee/__gruve/api") {
        return Err("Open the host’s 1kEE tile and copy its viewer address".into());
    }
    Ok(url
        .as_str()
        .trim_end_matches('/')
        .trim_end_matches("/index.html")
        .to_owned())
}

#[derive(Default)]
struct Update {
    image: Option<egui::ColorImage>,
    live: bool,
    epoch: u64,
    status: String,
}
struct Link {
    output: Arc<Mutex<Update>>,
    commands: Sender<(u64, Navigation)>,
    pending: VecDeque<Navigation>,
    next_control: Instant,
    stopped: Arc<AtomicBool>,
}

impl Link {
    fn connect(base: String, ctx: egui::Context) -> Result<Self, String> {
        let client = Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(3))
            .build()
            .map_err(|e| e.to_string())?;
        let (commands, rx) = crossbeam_channel::bounded::<(u64, Navigation)>(0);
        let output = Arc::new(Mutex::new(Update {
            status: "Connecting…".into(),
            ..Update::default()
        }));
        let shared = output.clone();
        let stopped = Arc::new(AtomicBool::new(false));
        let stop_worker = stopped.clone();
        let controls = client.clone();
        let control_url = format!("{base}/map/control");
        let control_state = shared.clone();
        let control_ctx = ctx.clone();
        let control_stop = stopped.clone();
        std::thread::spawn(move || {
            for (epoch, action) in rx {
                let state = control_state.lock().unwrap();
                if control_stop.load(Ordering::Acquire) {
                    break;
                }
                if !state.live || epoch != state.epoch {
                    continue;
                }
                drop(state);
                // A failed response can still mean the host applied the gesture.
                let result = controls
                    .post(&control_url)
                    .header("Content-Type", "application/json")
                    .body(serde_json::to_vec(&action).unwrap())
                    .send()
                    .and_then(|r| r.error_for_status());
                if let Err(error) = result {
                    control_state.lock().unwrap().status = format!("Control failed · {error}");
                    control_ctx.request_repaint();
                }
            }
        });
        std::thread::spawn(move || run(client, &base, &shared, &ctx, &stop_worker));
        Ok(Self {
            output,
            commands,
            pending: VecDeque::new(),
            next_control: Instant::now(),
            stopped,
        })
    }

    fn queue(&mut self, action: Navigation) {
        // Combine successive input samples while a control request is in flight.
        // Keep pan and zoom in order because zoom affects pan sensitivity.
        if let Some(last) = self.pending.back_mut() {
            let merged = match (*last, action) {
                (Navigation::Pan { dx, dy }, Navigation::Pan { dx: x, dy: y }) => {
                    Some(Navigation::Pan {
                        dx: dx + x,
                        dy: dy + y,
                    })
                }
                (Navigation::Zoom { factor }, Navigation::Zoom { factor: f }) => {
                    Some(Navigation::Zoom { factor: factor * f })
                }
                _ => None,
            };
            if let Some(merged) = merged.filter(|n| n.valid()) {
                *last = merged;
                return;
            }
        }
        if self.pending.len() < 16 {
            self.pending.push_back(action);
        }
    }

    fn send_pending(&mut self, ctx: &egui::Context, epoch: u64) {
        if let Some(action) = self.pending.front().copied() {
            let now = Instant::now();
            if now >= self.next_control {
                match self.commands.try_send((epoch, action)) {
                    Ok(()) => {
                        self.pending.pop_front();
                    }
                    Err(TrySendError::Full(_)) => {}
                    Err(TrySendError::Disconnected(_)) => self.pending.clear(),
                }
                self.next_control = now + Duration::from_millis(40);
            }
            if !self.pending.is_empty() {
                ctx.request_repaint_after(self.next_control.saturating_duration_since(now));
            }
        }
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        // A disconnect must discard gestures still queued for the old host.
        self.stopped.store(true, Ordering::Release);
    }
}

fn run(
    client: Client,
    base: &str,
    output: &Mutex<Update>,
    ctx: &egui::Context,
    stopped: &AtomicBool,
) {
    let mut seen = None;
    while !stopped.load(Ordering::Acquire) {
        let start = Instant::now();
        let frame = fetch(&client, base, seen);
        let mut update = output.lock().unwrap();
        match frame {
            Ok(Some((seq, image))) => {
                seen = Some(seq);
                update.image = Some(image);
                update.live = true;
                update.status = "Live".into();
            }
            Ok(None) => {}
            Err(message) => {
                seen = None;
                if update.live {
                    update.epoch += 1;
                }
                update.live = false;
                update.image = None;
                update.status = message;
            }
        }
        let live = update.live;
        drop(update);
        ctx.request_repaint();
        // New hosts wait for the next capture. A minimum request interval also
        // paces immediate replies from hosts which ignore the wait parameter.
        let interval = Duration::from_millis(if live { 40 } else { 750 });
        std::thread::sleep(interval.saturating_sub(start.elapsed()));
    }
}

fn fetch(
    client: &Client,
    base: &str,
    seen: Option<u64>,
) -> Result<Option<(u64, egui::ColorImage)>, String> {
    let mut request = client.get(format!("{base}/map/frame?wait=1"));
    if let Some(seq) = seen {
        request = request.query(&[("gen", seq)]);
    }
    let response = request.send().map_err(|e| format!("Reconnecting · {e}"))?;
    match response.status().as_u16() {
        200 => {}
        304 if seen.is_some() => return Ok(None),
        202 => return Err("Waiting for a frame from the host".into()),
        403 => return Err("Sharing is off. Enable it in host Settings → Overview.".into()),
        409 => return Err("Sharing paused while the host has Settings open".into()),
        status => {
            return Err(format!(
                "Host returned HTTP {status}. Check the 1kEE tile address."
            ));
        }
    }
    let seq = response
        .headers()
        .get("X-Gen")
        .and_then(|h| h.to_str().ok()?.parse().ok())
        .ok_or("Missing frame number")?;
    const LIMIT: u64 = 8 * 1024 * 1024;
    let mut bytes = Vec::new();
    response
        .take(LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > LIMIT {
        return Err("Map frame exceeds the size limit".into());
    }
    let mut reader = image::ImageReader::with_format(Cursor::new(bytes), image::ImageFormat::Jpeg);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_EDGE);
    limits.max_image_height = Some(MAX_EDGE);
    reader.limits(limits);
    let rgb = reader
        .decode()
        .map_err(|e| format!("Invalid map frame · {e}"))?
        .into_rgb8();
    Ok(Some((
        seq,
        egui::ColorImage::from_rgb([rgb.width() as usize, rgb.height() as usize], &rgb),
    )))
}

pub(crate) struct Viewer {
    address: String,
    link: Option<Link>,
    texture: Option<egui::TextureHandle>,
    status: String,
    control_epoch: u64,
}

impl Viewer {
    pub(crate) fn new(ctx: &egui::Context, address: String) -> Self {
        let mut viewer = Self {
            address,
            link: None,
            texture: None,
            status: String::new(),
            control_epoch: 0,
        };
        if !viewer.address.is_empty() {
            viewer.connect(ctx);
        }
        viewer
    }
    fn connect(&mut self, ctx: &egui::Context) {
        self.texture = None;
        self.control_epoch = 0;
        self.link = None;
        match endpoint(&self.address).and_then(|base| Link::connect(base, ctx.clone())) {
            Ok(link) => self.link = Some(link),
            Err(e) => self.status = e,
        }
    }
    fn steer(&mut self, action: Navigation) {
        if let Some(link) = &mut self.link {
            link.queue(action);
        }
    }
}

impl eframe::App for Viewer {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        if let Some(link) = &mut self.link {
            let mut state = link.output.lock().unwrap();
            if !state.live {
                self.texture = None;
            }
            if !state.live || self.control_epoch != state.epoch {
                link.pending.clear();
                self.control_epoch = state.epoch;
            }
            if let Some(image) = state.image.take() {
                if let Some(tex) = &mut self.texture {
                    tex.set(image, egui::TextureOptions::LINEAR);
                } else {
                    self.texture =
                        Some(ctx.load_texture("shared-map", image, egui::TextureOptions::LINEAR));
                }
            }
            self.status.clone_from(&state.status);
        }
        egui::TopBottomPanel::top("connection").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label("Shared map");
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut self.address)
                        .desired_width(330.)
                        .hint_text("Gruve tile address or local control port"),
                );
                if ui.button("Connect").clicked()
                    || edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))
                {
                    self.connect(ctx);
                }
                if self.link.is_some() && ui.button("Disconnect").clicked() {
                    self.link = None;
                    self.texture = None;
                    self.status = "Disconnected".into();
                }
                ui.label(&self.status);
            });
        });
        egui::CentralPanel::default().show(ctx, |ui| {
            let Some(texture) = &self.texture else { return };
            let available = ui.available_size();
            let size = texture.size_vec2();
            let scale = (available.x / size.x).min(available.y / size.y);
            let rect = egui::Rect::from_center_size(ui.max_rect().center(), size * scale);
            let response = ui.put(
                rect,
                egui::Image::new(texture)
                    .fit_to_exact_size(rect.size())
                    .sense(egui::Sense::drag()),
            );
            if response.dragged() {
                let drag = response.drag_delta();
                if drag != egui::Vec2::ZERO {
                    self.steer(Navigation::Pan {
                        dx: (drag.x / rect.width()).clamp(-1., 1.),
                        dy: (drag.y / rect.height()).clamp(-1., 1.),
                    });
                }
            }
            if response.hovered() {
                let scroll = ui.input(|i| i.raw_scroll_delta.y);
                if scroll != 0. {
                    self.steer(Navigation::Zoom {
                        factor: (scroll * 0.0055).exp().clamp(0.25, 4.),
                    });
                }
                response.on_hover_cursor(egui::CursorIcon::Grab);
            }
        });
        if let Some(link) = &mut self.link {
            link.send_pending(ctx, self.control_epoch);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forwarded_ports_and_gruve_tile_urls_address_the_same_api_as_the_page() {
        assert_eq!(endpoint(" 9768 ").unwrap(), "http://127.0.0.1:9768");
        assert_eq!(
            endpoint("localhost:63001").unwrap(),
            "http://localhost:63001"
        );
        let tile = "http://127.0.0.1:8088/peer/network/node/apps/1kee";
        let api = format!("{tile}/__gruve/api");
        assert_eq!(endpoint(&format!("{tile}/?gruve-solo=1")).unwrap(), api);
        assert_eq!(endpoint(&api).unwrap(), api);
        assert!(endpoint("0").is_err());
        assert!(endpoint("http://127.0.0.1:8088/api/mesh").is_err());
        assert!(endpoint("http://203.0.113.9:9766").is_err());
    }
}
