//! Event-pinned, opt-in public radio browser and small native player.
use crate::model::EventRecord;
use crate::radio::{
    RadioArea, RadioFeed, RadioKind, catalog,
    player::{PlaybackStatus, RadioPlayer},
    saved, stream_url,
};
use crate::theme;
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

struct Search {
    receiver: crossbeam_channel::Receiver<Result<catalog::CatalogReport, String>>,
    cancel: Arc<AtomicBool>,
}
impl Drop for Search {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}

pub struct EventRadio {
    open: bool,
    area: Option<RadioArea>,
    catalog: Vec<RadioFeed>,
    saved: Vec<RadioFeed>,
    loaded: bool,
    path: Option<PathBuf>,
    pending: Option<Search>,
    fetched_at: Option<Instant>,
    status: String,
    warning: Option<String>,
    error: Option<String>,
    radius: u16,
    kind: Option<RadioKind>,
    filter: String,
    browse_all: bool,
    player: RadioPlayer,
    playing: Option<RadioFeed>,
    volume: f32,
    add_open: bool,
    new_name: String,
    new_url: String,
    new_kind: RadioKind,
}

impl Default for EventRadio {
    fn default() -> Self {
        Self {
            open: false,
            area: None,
            catalog: Vec::new(),
            saved: Vec::new(),
            loaded: false,
            path: if cfg!(test) { None } else { saved::path() },
            pending: None,
            fetched_at: None,
            status: String::new(),
            warning: None,
            error: None,
            radius: 100,
            kind: None,
            filter: String::new(),
            browse_all: false,
            player: RadioPlayer::default(),
            playing: None,
            volume: 0.6,
            add_open: false,
            new_name: String::new(),
            new_url: String::new(),
            new_kind: RadioKind::PublicSafety,
        }
    }
}

impl EventRadio {
    pub(crate) fn title_bar_button(
        &mut self,
        ctx: &egui::Context,
        window: &egui::Response,
        event: &EventRecord,
    ) {
        let style = ctx.style();
        let frame = egui::Frame::window(&style);
        let heading = egui::TextStyle::Heading.resolve(&style);
        let height = ctx
            .fonts(|f| f.row_height(&heading))
            .max(style.spacing.interact_size.y)
            + frame.inner_margin.sum().y;
        let center = egui::pos2(
            window.rect.right() - frame.stroke.width - height / 2.0 - 29.0,
            window.rect.top() + frame.stroke.width + height / 2.0,
        );
        let rect = egui::Rect::from_center_size(center, egui::vec2(24.0, 24.0));
        let id = window.id.with("nearby-radio-title-action");
        let ui = egui::Ui::new(
            ctx.clone(),
            id.with("container"),
            egui::UiBuilder::new()
                .layer_id(window.layer_id)
                .max_rect(rect),
        );
        let response = ui
            .interact(rect, id, egui::Sense::click())
            .on_hover_text("Nearby emergency radio");
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Nearby emergency radio")
        });
        let visuals = ui.style().interact(&response);
        if response.hovered() {
            ui.painter().rect_filled(rect, 4, visuals.bg_fill);
        }
        let stroke = egui::Stroke::new(1.4, visuals.fg_stroke.color);
        let body =
            egui::Rect::from_center_size(center + egui::vec2(0.0, 2.0), egui::vec2(16.0, 11.0));
        ui.painter()
            .rect_stroke(body, 2, stroke, egui::StrokeKind::Inside);
        ui.painter().line_segment(
            [
                body.left_top() + egui::vec2(1.0, -1.0),
                center + egui::vec2(6.0, -9.0),
            ],
            stroke,
        );
        ui.painter()
            .circle_stroke(body.left_center() + egui::vec2(4.5, 0.0), 2.2, stroke);
        for y in [-2.0, 1.0] {
            ui.painter().line_segment(
                [
                    body.right_center() + egui::vec2(-5.5, y),
                    body.right_center() + egui::vec2(-2.0, y),
                ],
                stroke,
            );
        }
        if response.clicked() {
            self.open_for(ctx, event);
        }
    }

    pub(crate) fn button(&mut self, ui: &mut egui::Ui, event: &EventRecord) {
        if ui
            .button("Nearby radio")
            .on_hover_text("Find public police, fire, EMS and other radio near this event")
            .clicked()
        {
            self.open_for(ui.ctx(), event);
        }
    }

    pub(crate) fn open_for(&mut self, ctx: &egui::Context, event: &EventRecord) {
        let next = RadioArea::from(event);
        if self
            .area
            .as_ref()
            .is_none_or(|area| area.event_id != next.event_id || area.location != next.location)
        {
            self.stop();
            self.kind = None;
            self.filter.clear();
            self.browse_all = false;
        }
        self.area = Some(next);
        self.open = true;
        if !self.loaded {
            self.loaded = true;
            if let Some(path) = &self.path {
                match saved::load(path) {
                    Ok(feeds) => self.saved = feeds,
                    Err(e) => self.error = Some(e),
                }
            }
        }
        if self.pending.is_none()
            && self
                .fetched_at
                .is_none_or(|t| t.elapsed() > Duration::from_secs(900))
        {
            self.search(ctx);
        }
    }

    fn search(&mut self, ctx: &egui::Context) {
        if self.pending.is_some() {
            return;
        }
        self.status = "Searching public radio sources…".into();
        self.warning = None;
        self.error = None;
        let (sender, receiver) = crossbeam_channel::bounded(1);
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let result = catalog::fetch(&worker_cancel);
            if !worker_cancel.load(Ordering::Acquire) {
                let _ = sender.try_send(result);
                repaint.request_repaint();
            }
        });
        self.pending = Some(Search { receiver, cancel });
    }

    fn poll(&mut self) {
        if let Some(search) = &self.pending {
            match search.receiver.try_recv() {
                Ok(Ok(report)) => {
                    self.catalog = report.feeds;
                    self.warning = report.warning;
                    self.fetched_at = Some(Instant::now());
                    let weather = self
                        .catalog
                        .iter()
                        .filter(|feed| feed.kinds == [RadioKind::Weather])
                        .count();
                    self.status = format!(
                        "{} scanner/dispatch · {} weather feeds · cached for 15 minutes",
                        self.catalog.len() - weather,
                        weather
                    );
                    self.pending = None;
                }
                Ok(Err(error)) => {
                    self.error = Some(error);
                    self.status = "Search unavailable".into();
                    self.pending = None;
                }
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    self.error =
                        Some("Radio search stopped before returning results. Try Refresh.".into());
                    self.pending = None;
                }
                Err(crossbeam_channel::TryRecvError::Empty) => {}
            }
        }
        self.player.tick();
    }

    fn visible_feeds(&self, area: &RadioArea) -> Vec<(RadioFeed, Option<f64>)> {
        let mut feeds: Vec<_> = self
            .saved
            .iter()
            .chain(self.catalog.iter())
            .filter_map(|feed| {
                let distance = feed.distance_km(area);
                if !self.browse_all && distance.is_none_or(|d| d > self.radius as f64) {
                    return None;
                }
                if self.kind.is_some_and(|kind| !feed.kinds.contains(&kind)) {
                    return None;
                }
                if !self.filter.is_empty()
                    && !format!("{} {}", feed.name, feed.locality)
                        .to_lowercase()
                        .contains(&self.filter.to_lowercase())
                {
                    return None;
                }
                Some((feed.clone(), distance))
            })
            .collect();
        feeds.sort_by(|a, b| {
            a.1.unwrap_or(f64::INFINITY)
                .total_cmp(&b.1.unwrap_or(f64::INFINITY))
                .then(a.0.name.cmp(&b.0.name))
        });
        let mut urls = std::collections::HashSet::new();
        feeds.retain(|(feed, _)| urls.insert(feed.url.clone()));
        feeds
    }

    fn stop(&mut self) {
        self.player.stop();
        self.playing = None;
    }
    fn close(&mut self) {
        self.open = false;
        self.pending = None;
        self.stop();
    }

    pub fn render(&mut self, ctx: &egui::Context, selected: Option<&EventRecord>) {
        if !self.open {
            return;
        }
        self.poll();
        let Some(area) = self.area.clone() else {
            self.close();
            return;
        };
        let mut open = self.open;
        egui::Window::new("Nearby radio")
            .id(egui::Id::new("event-radio"))
            .open(&mut open)
            .default_width(480.0)
            .default_height(500.0)
            .min_width(360.0)
            .resizable(true)
            .vscroll(true)
            .frame(egui::Frame::window(&ctx.style()))
            .show(ctx, |ui| {
                self.contents(ui, &area, selected);
            });
        if !open {
            self.close();
        }
        if self.pending.is_some() || self.playing.is_some() {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
    }

    fn contents(&mut self, ui: &mut egui::Ui, area: &RadioArea, selected: Option<&EventRecord>) {
        ui.strong(&area.title);
        ui.label(format!(
            "{} · {:.3}, {:.3}",
            area.name, area.location.lat, area.location.lon
        ));
        if let Some(selected) = selected
            && selected.id != area.event_id
        {
            ui.horizontal_wrapped(|ui| {
                ui.small("Pinned to the searched event.");
                if ui.small_button("Search selected event").clicked() {
                    self.open_for(ui.ctx(), selected);
                }
            });
        }
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            ui.label("Radius");
            egui::ComboBox::from_id_salt("radio-radius")
                .selected_text(format!("{} km", self.radius))
                .width(75.0)
                .show_ui(ui, |ui| {
                    for radius in [25, 50, 100, 250, 500] {
                        ui.selectable_value(&mut self.radius, radius, format!("{radius} km"));
                    }
                });
            egui::ComboBox::from_id_salt("radio-category")
                .selected_text(self.kind.map_or("All types", RadioKind::label))
                .width(110.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.kind, None, "All types");
                    for kind in RadioKind::ALL {
                        ui.selectable_value(&mut self.kind, Some(kind), kind.label());
                    }
                });
            if ui
                .add_enabled(self.pending.is_none(), egui::Button::new("Refresh"))
                .clicked()
            {
                self.search(ui.ctx());
            }
        });
        ui.add(
            egui::TextEdit::singleline(&mut self.filter)
                .hint_text("Filter feeds by name or place")
                .desired_width(f32::INFINITY),
        );
        ui.checkbox(
            &mut self.browse_all,
            "Browse all open feeds, including outside this area",
        );
        ui.small("Station/coverage locations are approximate; proximity does not confirm coverage of this incident.");
        self.player_controls(ui);
        if self.pending.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.small(&self.status);
            });
        } else if !self.status.is_empty() {
            ui.small(&self.status);
        }
        if let Some(warning) = &self.warning {
            ui.colored_label(theme::text_muted(), warning);
        }
        if let Some(error) = &self.error {
            ui.colored_label(egui::Color32::LIGHT_RED, error);
        }
        if ui
            .selectable_label(self.add_open, "Add a public stream…")
            .clicked()
        {
            self.add_open = !self.add_open;
        }
        if self.add_open {
            self.add_stream(ui, area);
        }
        let feeds = self.visible_feeds(area);
        ui.separator();
        ui.label(format!(
            "{} matching feed{}",
            feeds.len(),
            if feeds.len() == 1 { "" } else { "s" }
        ));
        egui::ScrollArea::vertical().id_salt("radio-feeds").max_height(210.0).auto_shrink([false, true]).show(ui, |ui| {
            if feeds.is_empty() {
                ui.label(if self.pending.is_some() { "Searching…" } else { "No open feeds match this area and filter." });
                ui.small("Public police, fire and EMS coverage is limited. Try a wider radius, browse all feeds, or add a public stream.");
            }
            let mut remove = None;
            for (feed, distance) in feeds {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.strong(&feed.name);
                    ui.horizontal_wrapped(|ui| {
                        ui.small(feed.kinds.iter().map(|k| k.label()).collect::<Vec<_>>().join(" / "));
                        ui.small(distance.map(|d| format!("{d:.0} km away")).unwrap_or_else(|| "Location not mapped".into()));
                    });
                    if !feed.locality.is_empty() { ui.small(&feed.locality); }
                    ui.horizontal(|ui| {
                        if ui.button("Play").clicked() {
                            self.player.play(&feed.url, self.volume); self.playing = Some(feed.clone());
                            if let Some(uuid) = &feed.directory_id { catalog::record_play(uuid.clone()); }
                        }
                        if stream_url(&feed.homepage).is_ok() { ui.hyperlink_to("Source website", &feed.homepage); }
                        ui.small(feed.source_label());
                        if feed.id.starts_with("saved:") && ui.small_button("Remove").clicked() {
                            remove = Some(feed.id.clone());
                        }
                    });
                });
                ui.add_space(4.0);
            }
            if let Some(id) = remove {
                let next: Vec<_> = self.saved.iter().filter(|feed| feed.id != id).cloned().collect();
                if self.persist(&next) {
                    if self.playing.as_ref().is_some_and(|feed| feed.id == id) { self.stop(); }
                    self.saved = next;
                }
            }
        });
        ui.separator();
        ui.horizontal_wrapped(|ui| {
            ui.small("Sources:");
            ui.hyperlink_to("Radio Browser", "https://www.radio-browser.info/");
            ui.hyperlink_to("WeatherRadio", "https://www.weatherradio.org/");
            ui.hyperlink_to("NCISS", "https://nciss.live/");
            ui.hyperlink_to("ScanBC", "https://www.scanbc.com/");
            ui.small("Broader dispatch coverage:");
            ui.hyperlink_to("Broadcastify", "https://www.broadcastify.com/listen/");
            ui.hyperlink_to("OpenMHz", "https://openmhz.com/");
        });
        ui.horizontal_wrapped(|ui| {
            ui.small("Area centers:");
            ui.hyperlink_to("© OpenStreetMap contributors", "https://www.openstreetmap.org/copyright");
            ui.hyperlink_to("US Census", "https://www.census.gov/geographies/reference-files/time-series/geo/gazetteer-files.html");
        });
    }

    fn player_controls(&mut self, ui: &mut egui::Ui) {
        let Some(feed) = &self.playing else {
            return;
        };
        ui.add_space(6.0);
        ui.strong(format!("Listening · {}", feed.name));
        let status = self
            .player
            .status
            .clone()
            .unwrap_or(PlaybackStatus::Stopped);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    !matches!(status, PlaybackStatus::Failed(_) | PlaybackStatus::Stopped),
                    egui::Button::new(if status == PlaybackStatus::Paused {
                        "Resume"
                    } else {
                        "Pause"
                    }),
                )
                .clicked()
            {
                if status == PlaybackStatus::Paused {
                    self.player.resume();
                } else {
                    self.player.pause();
                }
            }
            if ui.button("Stop").clicked() {
                self.stop();
            }
            if ui
                .add(
                    egui::Slider::new(&mut self.volume, 0.0..=1.0)
                        .show_value(false)
                        .text("Volume"),
                )
                .changed()
            {
                self.player.volume(self.volume);
            }
            ui.small(status.label());
        });
        if let PlaybackStatus::Failed(error) = status {
            ui.colored_label(egui::Color32::LIGHT_RED, error);
        }
    }

    fn persist(&mut self, feeds: &[RadioFeed]) -> bool {
        match self
            .path
            .as_deref()
            .ok_or_else(|| "The local radio feed storage path is unavailable.".to_string())
            .and_then(|path| saved::save(path, feeds))
        {
            Ok(()) => {
                self.error = None;
                true
            }
            Err(e) => {
                self.error = Some(e);
                false
            }
        }
    }

    fn add_stream(&mut self, ui: &mut egui::Ui, area: &RadioArea) {
        ui.add(
            egui::TextEdit::singleline(&mut self.new_name)
                .hint_text("Feed name")
                .desired_width(f32::INFINITY),
        );
        ui.add(
            egui::TextEdit::singleline(&mut self.new_url)
                .hint_text("https://… direct audio stream")
                .desired_width(f32::INFINITY),
        );
        ui.horizontal_wrapped(|ui| {
            egui::ComboBox::from_id_salt("saved-radio-category")
                .selected_text(self.new_kind.label())
                .show_ui(ui, |ui| {
                    for kind in RadioKind::ALL {
                        ui.selectable_value(&mut self.new_kind, kind, kind.label());
                    }
                });
            if ui.button("Save at this event location").clicked() {
                match self.new_feed(area) {
                    Ok(feed) => {
                        let mut next = self.saved.clone();
                        next.retain(|old| old.url != feed.url);
                        next.push(feed);
                        if self.persist(&next) {
                            self.saved = next;
                            self.new_name.clear();
                            self.new_url.clear();
                            self.add_open = false;
                        }
                    }
                    Err(e) => self.error = Some(e),
                }
            }
        });
        ui.small("Use a stream you are permitted to play. Its location is your pin, not a measured coverage area.");
    }

    fn new_feed(&self, area: &RadioArea) -> Result<RadioFeed, String> {
        if self.new_name.trim().is_empty() {
            return Err("Give the radio feed a name.".into());
        }
        let url = stream_url(&self.new_url)?.to_string();
        Ok(RadioFeed {
            id: format!("saved:{url}"),
            name: self.new_name.trim().chars().take(200).collect(),
            url,
            homepage: String::new(),
            kinds: vec![self.new_kind],
            latitude: Some(area.location.lat as f64),
            longitude: Some(area.location.lon as f64),
            locality: format!("{} (user-pinned)", area.name),
            directory_id: None,
        })
    }
}

#[cfg(test)]
#[path = "event_radio_tests.rs"]
mod tests;
