//! Recent provider frames on one UTC timeline for both Earth views.
use super::{Feed, Product, SatelliteImage, SatelliteLayer, cache, provider};
use std::{
    collections::VecDeque,
    path::Path,
    sync::mpsc,
    time::{Duration, Instant},
};

// One active request per product, including during rapid seeks.
type ImageResult = Result<SatelliteImage, String>;

#[derive(Default)]
pub(super) struct Archive {
    loaded: VecDeque<(String, SatelliteLayer)>,
    worker: Option<(String, mpsc::Receiver<ImageResult>)>,
    failure: Option<(String, String)>,
}

impl Archive {
    fn poll(&mut self, ctx: &egui::Context, product: Product, keep: Option<&str>) {
        let Some((stamp, rx)) = &self.worker else {
            return;
        };
        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => Err("Playback worker stopped".into()),
        };
        match result {
            Ok(image) => self.remember(
                stamp.clone(),
                SatelliteLayer::new(ctx, product.label().into(), image),
                keep,
            ),
            Err(e) => self.failure = Some((stamp.clone(), e)),
        }
        self.worker = None;
    }

    pub(super) fn remember(&mut self, stamp: String, image: SatelliteLayer, keep: Option<&str>) {
        self.loaded.retain(|(time, _)| *time != stamp);
        if self.loaded.len() >= 3
            && let Some(i) = self
                .loaded
                .iter()
                .position(|(t, _)| Some(t.as_str()) != keep)
        {
            self.loaded.remove(i);
        }
        self.loaded.push_back((stamp, image));
    }

    pub(super) fn idle(&mut self) {
        self.loaded.clear();
        self.failure = None;
        // Let an outstanding download finish and cache its PNG. Re-entering
        // playback immediately still respects the single-worker limit.
        if self
            .worker
            .as_ref()
            .is_some_and(|(_, rx)| !matches!(rx.try_recv(), Err(mpsc::TryRecvError::Empty)))
        {
            self.worker = None;
        }
    }

    fn request(&mut self, ctx: &egui::Context, product: Product, stamp: &str, path: &Path) {
        if self.worker.is_some() || self.error(stamp).is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let time = stamp.to_owned();
        let path = path.to_owned();
        let max_side = ctx.input(|i| i.max_texture_side);
        let wake = ctx.clone();
        match std::thread::Builder::new()
            .name("weather-playback".into())
            .spawn(move || {
                let result = load_frame(product, &time, &path, max_side);
                let _ = tx.send(result);
                wake.request_repaint();
            }) {
            Ok(_) => self.worker = Some((stamp.to_owned(), rx)),
            Err(e) => self.failure = Some((stamp.to_owned(), e.to_string())),
        }
    }

    fn error(&self, stamp: &str) -> Option<&str> {
        self.failure
            .as_ref()
            .filter(|(t, _)| t == stamp)
            .map(|(_, e)| e.as_str())
    }
}

fn load_frame(product: Product, time: &str, path: &Path, max_side: usize) -> ImageResult {
    if let Some(png) = cache::historical_png(path, product, time)
        && let Ok(image) = provider::decode(product, &png, max_side)
    {
        return Ok(image);
    }
    let png = provider::fetch_png(&provider::client()?, product, time)?;
    let image = provider::decode(product, &png, max_side)?;
    let _ = cache::store_history(path, product, time, &png);
    Ok(image)
}

impl Feed {
    pub(super) fn frame_at(&self, stamp: &str) -> Option<&SatelliteLayer> {
        if self.stamp.as_deref() == Some(stamp) {
            return self.image.as_ref();
        }
        self.archive
            .loaded
            .iter()
            .find(|(t, _)| t == stamp)
            .map(|(_, img)| img)
    }
    fn prepare(&mut self, ctx: &egui::Context, path: &Path, stamp: &str) -> bool {
        if self.frame_at(stamp).is_some() {
            true
        } else {
            self.archive.request(ctx, self.product, stamp, path);
            false
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
struct Step {
    time: String,
    stamps: [Option<String>; 2],
}

fn timeline(feeds: &[Feed; 2], days: u32) -> Vec<Step> {
    let [color, ir] = feeds;
    let has_ir = ir.options.enabled && !ir.catalog.is_empty();
    let primary = if has_ir { ir } else { color };
    if !primary.options.enabled {
        return Vec::new();
    }
    let Some(latest) = primary
        .catalog
        .last()
        .and_then(|s| provider::parse_stamp(primary.product, s))
    else {
        return Vec::new();
    };
    let cutoff = latest - i64::from(days) * 86400;
    primary
        .catalog
        .iter()
        .filter(|s| provider::parse_stamp(primary.product, s).is_some_and(|t| t >= cutoff))
        .filter_map(|time| {
            let date = time.get(..10)?;
            let daily = color.options.enabled
                && color
                    .catalog
                    .binary_search_by(|s| s.as_str().cmp(date))
                    .is_ok();
            Some(Step {
                time: time.clone(),
                stamps: [daily.then(|| date.to_owned()), has_ir.then(|| time.clone())],
            })
        })
        .collect()
}

pub(super) fn label(stamp: &str) -> String {
    if stamp.len() == 10 {
        format!("{stamp} UTC · daily mosaic")
    } else {
        let time = stamp.trim_end_matches('Z');
        format!(
            "{} UTC",
            time.strip_suffix(":00").unwrap_or(time).replace('T', " ")
        )
    }
}

pub(super) struct Player {
    steps: Vec<Step>,
    inputs: [(bool, u64); 2],
    days: u32,
    target: usize,
    shown: Option<usize>,
    pub playing: bool,
    speed: f32,
    shown_at: Instant,
}

impl Player {
    pub fn new(feeds: &[Feed; 2]) -> Self {
        let mut player = Self {
            steps: Vec::new(),
            inputs: [(false, 0); 2],
            days: 3,
            target: 0,
            shown: None,
            playing: true,
            speed: 1.,
            shown_at: Instant::now(),
        };
        player.rebuild(feeds);
        player
    }

    fn rebuild(&mut self, feeds: &[Feed; 2]) {
        let steps = timeline(feeds, self.days);
        let old = self.steps.get(self.target);
        self.target = old
            .map_or(0, |s| steps.partition_point(|step| step.time < s.time))
            .min(steps.len().saturating_sub(1));
        self.shown = self.shown.and_then(|i| {
            steps
                .iter()
                .position(|step| self.steps.get(i) == Some(step))
        });
        self.steps = steps;
        self.inputs = std::array::from_fn(|i| (feeds[i].options.enabled, feeds[i].catalog_version));
    }

    pub fn showing(&self, layer: usize) -> Option<&str> {
        self.steps.get(self.shown?)?.stamps[layer].as_deref()
    }

    pub fn shown_time(&self) -> Option<&str> {
        Some(&self.steps.get(self.shown?)?.time)
    }

    fn next(&self) -> usize {
        (self.target + 1) % self.steps.len()
    }

    pub fn tick(&mut self, ctx: &egui::Context, feeds: &mut [Feed; 2], path: &Path) {
        if self.inputs
            != std::array::from_fn(|i| (feeds[i].options.enabled, feeds[i].catalog_version))
        {
            self.rebuild(feeds);
        }
        for (i, feed) in feeds.iter_mut().enumerate() {
            feed.archive.poll(ctx, feed.product, self.showing(i));
            for (_, image) in &mut feed.archive.loaded {
                image.set_opacity(feed.options.opacity);
            }
        }
        let Some(step) = self.steps.get(self.target) else {
            return;
        };
        let mut ready = true;
        for (feed, stamp) in feeds.iter_mut().zip(&step.stamps) {
            if let Some(stamp) = stamp {
                ready &= feed.prepare(ctx, path, stamp);
                if feed.archive.error(stamp).is_some() {
                    self.playing = false;
                }
            }
        }
        if !ready {
            return;
        }
        let now = Instant::now();
        if self.shown != Some(self.target) {
            self.shown = Some(self.target);
            self.shown_at = now;
        }
        if !self.playing || self.steps.len() < 2 {
            return;
        }
        let next = self.next();
        for (feed, stamp) in feeds.iter_mut().zip(&self.steps[next].stamps) {
            if let Some(stamp) = stamp {
                feed.prepare(ctx, path, stamp);
            }
        }
        let wait = Duration::from_secs_f32(1. / self.speed).saturating_sub(now - self.shown_at);
        if wait.is_zero() {
            self.target = next;
        }
        ctx.request_repaint_after(wait);
    }

    pub fn controls(&mut self, ui: &mut egui::Ui, feeds: &mut [Feed; 2]) {
        ui.horizontal(|ui| {
            ui.label("Range");
            for (days, label) in [(1, "24 h"), (3, "3 days"), (7, "7 days")] {
                if ui.selectable_value(&mut self.days, days, label).changed() {
                    self.rebuild(feeds);
                }
            }
        });
        let Some(step) = self.steps.get(self.target) else {
            ui.small("Waiting for available frame times…");
            return;
        };
        // Keep the transport controls in place while the next frame buffers.
        let selected = label(&step.time);
        ui.small(if self.shown == Some(self.target) {
            selected
        } else {
            format!("Selected {selected}")
        });
        for (feed, stamp) in feeds.iter_mut().zip(&step.stamps) {
            if let Some(error) = stamp.as_deref().and_then(|t| feed.archive.error(t)) {
                ui.small(format!("{} unavailable · {error}", feed.product.label()));
                if ui.small_button("Retry frame").clicked() {
                    feed.archive.failure = None;
                }
            }
        }
        let last = self.steps.len() - 1;
        if ui
            .add_enabled(
                last > 0,
                egui::Slider::new(&mut self.target, 0..=last).show_value(false),
            )
            .on_hover_text(format!(
                "{} — {}",
                label(&self.steps[0].time),
                label(&self.steps[last].time)
            ))
            .changed()
        {
            self.playing = false;
        }
        ui.horizontal(|ui| {
            if ui
                .add_enabled(self.target > 0, egui::Button::new("Prev"))
                .clicked()
            {
                self.target -= 1;
                self.playing = false;
            }
            if ui
                .add_enabled(
                    last > 0,
                    egui::Button::new(if self.playing { "Pause" } else { "Play" }),
                )
                .clicked()
            {
                self.playing = !self.playing;
                self.shown_at = Instant::now();
            }
            if ui
                .add_enabled(self.target < last, egui::Button::new("Next"))
                .clicked()
            {
                self.target += 1;
                self.playing = false;
            }
            egui::ComboBox::from_id_salt("weather-speed")
                .width(44.)
                .selected_text(format!("{}×", self.speed))
                .show_ui(ui, |ui| {
                    for speed in [0.5, 1., 2.] {
                        ui.selectable_value(&mut self.speed, speed, format!("{speed}×"));
                    }
                })
                .response
                .on_hover_text("Playback speed · 1× is one frame per second");
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panels::world_map::global_weather::{GlobalWeather, Settings};

    const IR: [&str; 3] = [
        "2026-11-07T21:00:00Z",
        "2026-11-08T00:00:00Z",
        "2026-11-08T03:00:00Z",
    ];

    fn fixture(path: &Path) -> GlobalWeather {
        let mut weather = GlobalWeather::new(Settings::default(), path.to_owned());
        for feed in &mut weather.feeds {
            feed.options.enabled = true;
        }
        weather.feeds[0].catalog = vec!["2026-11-06".into(), "2026-11-07".into()];
        weather.feeds[1].catalog = IR.map(String::from).to_vec();
        weather
    }

    fn image(ctx: &egui::Context) -> SatelliteLayer {
        SatelliteLayer::new(
            ctx,
            "test".into(),
            SatelliteImage {
                bounds: [-180., -90., 180., 90.],
                pixels: egui::ColorImage::new([2, 1], egui::Color32::WHITE),
            },
        )
    }

    #[test]
    fn timeline_tracks_enabled_cadence_and_matches_daily_by_exact_date() {
        let mut weather = fixture(Path::new(""));
        let steps = timeline(&weather.feeds, 1);
        assert_eq!(steps.len(), 3);
        assert_eq!(
            steps[0].stamps,
            [Some("2026-11-07".into()), Some(IR[0].into())]
        );
        assert_eq!(steps[1].stamps, [None, Some(IR[1].into())]);
        weather.feeds[1].options.enabled = false;
        assert_eq!(
            timeline(&weather.feeds, 1)
                .iter()
                .map(|s| s.time.as_str())
                .collect::<Vec<_>>(),
            ["2026-11-06", "2026-11-07"]
        );
        weather.feeds[1].options.enabled = true;
        weather.feeds[1]
            .catalog
            .insert(0, "2026-11-01T21:00:00Z".into());
        assert_eq!(timeline(&weather.feeds, 3).len(), 3);
        assert_eq!(timeline(&weather.feeds, 7).len(), 4);
        assert_eq!(label(IR[1]), "2026-11-08 00:00 UTC");
    }

    #[test]
    fn advancing_waits_for_matching_pixels_and_live_mode_restores_latest() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("weather.sqlite");
        let mut weather = fixture(&path);
        let ctx = egui::Context::default();
        weather.feeds[0].stamp = Some("2026-11-07".into());
        weather.feeds[0].image = Some(image(&ctx));
        weather.feeds[1].stamp = Some(IR[2].into());
        weather.feeds[1].image = Some(image(&ctx));
        let ir = &mut weather.feeds[1].archive;
        ir.loaded.push_back((IR[0].into(), image(&ctx)));
        let (tx, rx) = mpsc::channel();
        ir.worker = Some((IR[1].into(), rx));
        let mut player = Player::new(&weather.feeds);
        player.tick(&ctx, &mut weather.feeds, &path);
        assert_eq!(player.shown_time(), Some(IR[0]));
        player.shown_at -= Duration::from_secs(2);
        player.tick(&ctx, &mut weather.feeds, &path);
        assert_eq!(player.target, 1);
        player.tick(&ctx, &mut weather.feeds, &path);
        assert_eq!(player.shown_time(), Some(IR[0]));
        tx.send(Err("offline".into())).unwrap();
        player.tick(&ctx, &mut weather.feeds, &path);
        assert!(!player.playing);
        assert_eq!(player.shown_time(), Some(IR[0]));
        // Seek over a failed frame. Only the matching IR image should be drawn.
        player.target = 2;
        player.tick(&ctx, &mut weather.feeds, &path);
        assert_eq!(player.showing(0), None);
        assert_eq!(player.shown_time(), Some(IR[2]));
        player.playing = true;
        player.shown_at -= Duration::from_secs(2);
        player.tick(&ctx, &mut weather.feeds, &path);
        assert_eq!(player.target, 0);
        player.tick(&ctx, &mut weather.feeds, &path);
        weather.player = Some(player);
        assert_eq!(weather.displayed(1).unwrap().0, IR[0]);
        weather.stop_playback();
        assert_eq!(weather.displayed(1).unwrap().0, IR[2]);
    }
}
