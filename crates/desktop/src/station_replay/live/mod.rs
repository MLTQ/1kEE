mod detect;
mod follow;
pub(super) mod seedlink;

use super::{Pick, StationId, day};
use crossbeam_channel::{Receiver, Sender, bounded};
use day::Inventory;
use detect::Motion;
use std::collections::HashMap;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

pub(crate) const FRESH_MS: i64 = 120_000;
pub(crate) const FLASH_MS: i64 = 20_000;
const PICK_LOOKBACK: i64 = 600;

#[derive(Default)]
struct Reading {
    sample_ms: i64,
    motion: Option<Motion>,
}

pub(crate) struct Trigger {
    pub observed_ms: i64,
    pub received_ms: i64,
    pub ratio: f64,
}

pub(crate) struct Report {
    pub pick: Pick,
    pub event: Arc<str>,
    pub received_ms: i64,
}

#[derive(Default)]
pub(crate) struct Activity {
    pub sample_ms: i64,
    pub motion: Option<Trigger>,
    pub report: Option<Report>,
}

enum Update {
    Stream(String),
    Readings(HashMap<StationId, Reading>),
    Inventory(Inventory, String),
    Picks(Arc<str>, i64, Vec<(StationId, Pick)>),
    Catalog(String),
}

struct Session {
    stop: Arc<AtomicBool>,
    rx: Receiver<Update>,
    started_ms: i64,
    stream: Option<StreamWorker>,
    tx: Sender<Update>,
}

struct StreamWorker(Arc<AtomicBool>);

impl Drop for StreamWorker {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

#[derive(Default)]
pub(crate) struct LiveStations {
    pub enabled: bool,
    pub stations: Inventory,
    pub activity: HashMap<StationId, Activity>,
    pub stream_status: String,
    pub inventory_status: String,
    pub catalog_status: String,
    pub reporting: usize,
    pub follow: follow::QuakeFollow,
    session: Option<Session>,
    next_count_ms: i64,
}

impl LiveStations {
    pub fn tick(&mut self, ctx: &egui::Context, following: bool) {
        let now = chrono::Utc::now().timestamp_millis();
        self.follow.set_enabled(following, now);
        if !self.enabled && !following {
            if self.session.is_some() {
                *self = Self::default();
            }
            return;
        }
        ctx.request_repaint_after(Duration::from_secs(1));
        let session = self.session.get_or_insert_with(|| start(ctx, now));
        if self.enabled {
            session
                .stream
                .get_or_insert_with(|| start_stream(ctx, session.tx.clone()));
        } else {
            session.stream = None;
        }
        while let Ok(update) = session.rx.try_recv() {
            match update {
                Update::Stream(s) => self.stream_status = s,
                Update::Catalog(s) => self.catalog_status = s,
                Update::Inventory(stations, status) => {
                    if !stations.is_empty() {
                        self.stations = stations;
                    }
                    self.inventory_status = status;
                }
                Update::Readings(readings) => {
                    for (id, reading) in readings {
                        let state = self.activity.entry(id).or_default();
                        state.sample_ms = state.sample_ms.max(reading.sample_ms);
                        if let Some(hit) = reading.motion
                            && hit.time_ms >= now - FRESH_MS
                            && state
                                .motion
                                .as_ref()
                                .is_none_or(|old| hit.time_ms > old.observed_ms)
                        {
                            state.motion = Some(Trigger {
                                observed_ms: hit.time_ms,
                                received_ms: now,
                                ratio: hit.ratio,
                            });
                        }
                    }
                }
                Update::Picks(event, origin, picks) => {
                    self.follow.observe(&event, origin, &picks);
                    for (id, pick) in picks {
                        // Opening the layer must not light up old arrivals from its bootstrap query.
                        if pick.time_ms < session.started_ms - FLASH_MS
                            || pick.time_ms < now - PICK_LOOKBACK * 1000
                            || pick.time_ms > now
                        {
                            continue;
                        }
                        let state = self.activity.entry(id).or_default();
                        if state
                            .report
                            .as_ref()
                            .is_none_or(|old| old.pick.time_ms < pick.time_ms)
                        {
                            state.report = Some(Report {
                                pick,
                                event: Arc::clone(&event),
                                received_ms: now,
                            });
                        }
                    }
                }
            }
        }
        if now >= self.next_count_ms {
            self.activity.retain(|_, a| {
                a.sample_ms >= now - FRESH_MS
                    || a.report.as_ref().is_some_and(|r| {
                        r.pick.time_ms >= now - PICK_LOOKBACK * 1000
                            || r.received_ms >= now - FLASH_MS
                    })
            });
            self.reporting = self
                .activity
                .iter()
                .filter(|(id, a)| {
                    a.sample_ms >= now - FRESH_MS
                        && self
                            .stations
                            .get(*id)
                            .and_then(|s| s.location(now))
                            .is_some()
                })
                .count();
            self.next_count_ms = now + 1000;
        }
    }
}

fn start(ctx: &egui::Context, started_ms: i64) -> Session {
    let (tx, rx) = bounded(128);
    let stop = Arc::new(AtomicBool::new(false));
    let archive_stop = Arc::clone(&stop);
    let archive_tx = tx.clone();
    let repaint = ctx.clone();
    std::thread::spawn(move || poll_catalog(&archive_stop, |u| emit(&archive_tx, &repaint, u)));
    Session {
        stop,
        rx,
        started_ms,
        stream: None,
        tx,
    }
}

fn start_stream(ctx: &egui::Context, stream_tx: Sender<Update>) -> StreamWorker {
    let stop = Arc::new(AtomicBool::new(false));
    let stream_stop = stop.clone();
    let repaint = ctx.clone();
    std::thread::spawn(move || {
        let send = |update| emit(&stream_tx, &repaint, update);
        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                send(Update::Stream(e.to_string()));
                return;
            }
        };
        while !stream_stop.load(Ordering::Relaxed) {
            send(Update::Stream("Connecting to EarthScope".into()));
            let result = runtime.block_on(seedlink::run(
                &stream_stop,
                |readings| send(Update::Readings(readings)),
                || send(Update::Stream("SeedLink connected".into())),
            ));
            if let Err(e) = result {
                send(Update::Stream(format!("Reconnecting · {e}")));
            }
            wait(&stream_stop, Duration::from_secs(30));
        }
    });
    StreamWorker(stop)
}

fn emit(tx: &Sender<Update>, ctx: &egui::Context, update: Update) {
    if tx.send(update).is_ok() {
        ctx.request_repaint();
    }
}

pub(super) fn wait(stop: &AtomicBool, duration: Duration) {
    let end = Instant::now() + duration;
    while !stop.load(Ordering::Relaxed) {
        let delay = end.saturating_duration_since(Instant::now());
        if delay.is_zero() {
            break;
        }
        std::thread::sleep(delay.min(Duration::from_millis(250)));
    }
}

fn poll_catalog(stop: &AtomicBool, send: impl Fn(Update) + Sync) {
    let mut next_inventory = 0;
    while !stop.load(Ordering::Relaxed) {
        let now = chrono::Utc::now().timestamp();
        if now >= next_inventory {
            let midnight = now.div_euclid(day::DAY) * day::DAY;
            let mut messages = Vec::new();
            let stations =
                day::load_inventory(midnight, midnight + day::DAY, stop, |w| messages.push(w))
                    .unwrap_or_else(|error| {
                        messages.push(error);
                        Inventory::new()
                    });
            next_inventory = if messages.is_empty() {
                midnight + day::DAY
            } else {
                now + 300
            };
            send(Update::Inventory(stations, messages.join(" · ")));
        }
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let from = now - PICK_LOOKBACK;
        let failed = std::sync::atomic::AtomicUsize::new(0);
        let result = day::load_picks(from, now, stop, &|update| match update {
            day::Update::Picks {
                id,
                origin_ms,
                picks,
            } => {
                let arrivals = picks
                    .into_iter()
                    .flat_map(|(station, picks)| {
                        picks
                            .into_iter()
                            .filter(move |p| {
                                p.time_ms <= now * 1000
                                    && (super::source::MIN_PICK_MS..=super::source::MAX_PICK_MS)
                                        .contains(&(p.time_ms - origin_ms))
                            })
                            .map(move |p| (station.clone(), p))
                    })
                    .collect::<Vec<_>>();
                if !arrivals.is_empty() {
                    send(Update::Picks(id.into(), origin_ms, arrivals));
                }
            }
            day::Update::Cooldown(_) => send(Update::Catalog(
                "USGS rate limited · waiting to retry".into(),
            )),
            day::Update::FailedEvent(_) => {
                failed.fetch_add(1, Ordering::Relaxed);
            }
            _ => {}
        });
        let failures = failed.load(Ordering::Relaxed);
        send(Update::Catalog(match result {
            Err(e) => format!("USGS picks unavailable · {e}"),
            Ok(()) if failures > 0 => {
                format!("USGS picks partial · {failures} quake downloads unavailable")
            }
            Ok(()) => "USGS picks checked".into(),
        }));
        wait(stop, Duration::from_secs(60));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skips_old_picks_and_only_flashes_a_report_once() {
        let now = chrono::Utc::now().timestamp_millis();
        let ctx = egui::Context::default();
        let (tx, rx) = bounded(8);
        let stop = Arc::new(AtomicBool::new(false));
        let mut layer = LiveStations {
            enabled: true,
            session: Some(Session {
                stop: stop.clone(),
                rx,
                started_ms: now,
                tx: tx.clone(),
                stream: Some(StreamWorker(Arc::new(AtomicBool::new(false)))),
            }),
            ..Default::default()
        };
        let id = super::super::source::station_id(Some("IU"), Some("SSPA")).unwrap();
        let send_pick = |time_ms| {
            let pick = Pick {
                phase: "P".into(),
                time_ms,
                used: true,
            };
            tx.send(Update::Picks(
                "quake".into(),
                now - 1000,
                vec![(id.clone(), pick)],
            ))
            .unwrap();
        };
        send_pick(now - 60_000);
        layer.tick(&ctx, false);
        assert!(layer.activity.is_empty());
        send_pick(now - 1_000);
        layer.tick(&ctx, false);
        let report = layer
            .activity
            .get_mut(&id)
            .unwrap()
            .report
            .as_mut()
            .unwrap();
        assert_eq!(&*report.event, "quake");
        report.received_ms = now - 5_000;
        send_pick(now - 1_000);
        layer.tick(&ctx, false);
        assert_eq!(
            layer.activity[&id].report.as_ref().unwrap().received_ms,
            now - 5_000
        );
        layer.enabled = false;
        layer.tick(&ctx, false);
        assert!(stop.load(Ordering::Relaxed));
        assert!(layer.activity.is_empty());
    }

    #[test]
    #[ignore = "requires EarthScope station inventory and live SeedLink"]
    fn passive_layer_maps_streamed_stations_and_stops_on_disable() {
        let mut layer = LiveStations {
            enabled: true,
            ..Default::default()
        };
        let ctx = egui::Context::default();
        let deadline = Instant::now() + Duration::from_secs(90);
        while Instant::now() < deadline {
            layer.tick(&ctx, false);
            if layer.reporting >= 1000 {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        eprintln!(
            "{} mapped reporting stations, {} listed ({}, {})",
            layer.reporting,
            layer.stations.len(),
            layer.stream_status,
            layer.inventory_status
        );
        assert!(layer.reporting >= 10);
        layer.enabled = false;
        layer.tick(&ctx, false);
        assert!(layer.session.is_none());
    }
}
