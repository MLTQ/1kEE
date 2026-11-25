mod cache;
mod load;
pub(super) use load::{load_inventory, load_picks};

use super::source::{MAX_PICK_MS, MIN_PICK_MS, Picks};
use super::{Pick, StationId};
use crate::model::GeoPoint;
use chrono::NaiveDate;
use crossbeam_channel::{Receiver, TryRecvError, unbounded};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub(crate) const DAY: i64 = 86_400;
const BINS: usize = 288;

pub(crate) struct Epoch {
    pub point: GeoPoint,
    start_ms: i64,
    end_ms: i64,
}

pub(crate) struct Arrival {
    pub pick: Pick,
    pub event: Arc<str>,
}

#[derive(Default)]
pub(crate) struct DayStation {
    epochs: Vec<Epoch>,
    arrivals: Vec<Arrival>,
}

impl DayStation {
    pub fn location(&self, time_ms: i64) -> Option<GeoPoint> {
        self.epochs
            .iter()
            .find(|epoch| epoch.start_ms <= time_ms && time_ms <= epoch.end_ms)
            .map(|epoch| epoch.point)
    }

    pub fn last_arrival(&self, time_ms: i64) -> Option<&Arrival> {
        let end = self
            .arrivals
            .partition_point(|arrival| arrival.pick.time_ms <= time_ms);
        self.arrivals[..end].last()
    }
}

pub(super) type Inventory = BTreeMap<StationId, DayStation>;

pub(super) enum Update {
    Inventory(Inventory),
    Catalog(usize),
    Picks {
        id: String,
        origin_ms: i64,
        picks: Picks,
    },
    FailedEvent(String),
    Warning(String),
    Cooldown(std::time::Instant),
    Done,
}

pub(crate) struct StationDay {
    pub date_entry: String,
    pub from: i64,
    pub to: i64,
    pub stations: Inventory,
    pub bins: [u32; BINS],
    pub mapped_picks: usize,
    pub unmapped_picks: usize,
    pub catalog_size: Option<usize>,
    pub processed: usize,
    pub failed: usize,
    pub warnings: Vec<String>,
    pub last_error: Option<String>,
    pub cooldown: Option<std::time::Instant>,
    updates: Option<Receiver<Update>>,
    cancelled: Arc<AtomicBool>,
}

impl Drop for StationDay {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

impl StationDay {
    pub fn open(date: NaiveDate, now: i64, ctx: &egui::Context) -> Self {
        let from = date.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp();
        let mut day = Self::new(from, (from + DAY).min(now).max(from + 1));
        let (tx, rx) = unbounded();
        day.updates = Some(rx);
        let cancelled = Arc::clone(&day.cancelled);
        let ctx = ctx.clone();
        let to = day.to;
        std::thread::spawn(move || {
            let send = |update| {
                if tx.send(update).is_ok() {
                    ctx.request_repaint();
                }
            };
            if let Err(error) = load::load(from, to, &cancelled, &send) {
                send(Update::Warning(error));
            }
            send(Update::Done);
        });
        day
    }

    fn new(from: i64, to: i64) -> Self {
        Self {
            date_entry: chrono::DateTime::from_timestamp(from, 0)
                .unwrap()
                .date_naive()
                .to_string(),
            from,
            to,
            stations: Inventory::new(),
            bins: [0; BINS],
            mapped_picks: 0,
            unmapped_picks: 0,
            catalog_size: None,
            processed: 0,
            failed: 0,
            warnings: Vec::new(),
            last_error: None,
            cooldown: None,
            updates: None,
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn loading(&self) -> bool {
        self.updates.is_some()
    }

    pub fn poll(&mut self) {
        while let Some(rx) = &self.updates {
            match rx.try_recv() {
                Ok(Update::Inventory(stations)) => self.stations = stations,
                Ok(Update::Catalog(size)) => self.catalog_size = Some(size),
                Ok(Update::Picks {
                    id,
                    origin_ms,
                    picks,
                }) => {
                    self.add_picks(id, origin_ms, picks);
                    self.processed += 1;
                }
                Ok(Update::FailedEvent(error)) => {
                    self.processed += 1;
                    self.failed += 1;
                    self.last_error = Some(error);
                }
                Ok(Update::Cooldown(until)) => self.cooldown = Some(until),
                Ok(Update::Warning(warning)) => self.warnings.push(warning),
                Ok(Update::Done) => self.updates = None,
                Err(TryRecvError::Disconnected) => {
                    self.warnings
                        .push("Station data download interrupted.".into());
                    self.updates = None;
                }
                Err(TryRecvError::Empty) => break,
            }
        }
    }

    fn add_picks(&mut self, id: String, origin_ms: i64, picks: Picks) {
        let event: Arc<str> = id.into();
        let start = self.from * 1000;
        let stop = self.to * 1000;
        for (id, mut picks) in picks {
            // Channel duplicates with different location weights represent one observed arrival.
            picks.sort_by(|a, b| (a.time_ms, &a.phase).cmp(&(b.time_ms, &b.phase)));
            picks.dedup_by(|a, b| a.time_ms == b.time_ms && a.phase == b.phase);
            picks.retain(|p| {
                (start..stop).contains(&p.time_ms)
                    && (MIN_PICK_MS..=MAX_PICK_MS).contains(&(p.time_ms - origin_ms))
            });
            let Some(station) = self.stations.get_mut(&id) else {
                self.unmapped_picks += picks.len();
                continue;
            };
            for pick in picks {
                if station.location(pick.time_ms).is_none() {
                    self.unmapped_picks += 1;
                    continue;
                }
                let bin = ((pick.time_ms - start) * BINS as i64 / (stop - start)) as usize;
                self.bins[bin] += 1;
                self.mapped_picks += 1;
                station.arrivals.push(Arrival {
                    pick,
                    event: Arc::clone(&event),
                });
            }
            // Concurrent event downloads can finish out of catalog order.
            station.arrivals.sort_by(|a, b| {
                (a.pick.time_ms, &a.event, &a.pick.phase).cmp(&(
                    b.pick.time_ms,
                    &b.event,
                    &b.pick.phase,
                ))
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::station_replay::{Phase, StationId};

    fn station(code: &str) -> StationId {
        StationId {
            network: "XX".into(),
            code: code.into(),
        }
    }

    fn pick(phase: &str, time_ms: i64) -> Pick {
        Pick {
            phase: phase.into(),
            time_ms,
            used: true,
        }
    }

    #[test]
    fn follow_stops_once_at_earliest_mapped_arrival_then_resumes_touring() {
        use crate::model::AppModel;
        let mut model = AppModel::new();
        model.factal_api_key.clear();
        model.events = vec![crate::model::EventRecord {
            id: "factal-live".into(),
            title: "Live report".into(),
            summary: String::new(),
            severity: crate::model::EventSeverity::Advisory,
            location_name: "City".into(),
            location: GeoPoint { lat: 51., lon: 0. },
            source: "Test".into(),
            occurred_at: String::new(),
            factal_brief: Some(crate::model::FactalBrief {
                factal_id: "live".into(),
                severity_value: None,
                occurred_at_raw: None,
                point_wkt: None,
                vertical: None,
                subvertical: None,
                topics: vec![],
                content: None,
                raw_json_pretty: String::new(),
            }),
        }];
        model.toggle_event_follow();
        model.tick_event_follow(0.);
        assert_eq!(model.selected_event_id.as_deref(), Some("factal-live"));
        model.tick_event_follow(301.);
        assert!(model.follow_recaps.active.is_some());
        // Deterministic origin clock. Picks arrive unordered, before coordinates.
        let start = chrono::Utc::now().timestamp_millis() + 1;
        let quake: Arc<str> = "test-quake".into();
        let picks = vec![
            (station("LATE"), pick("P", start + 3500)),
            (station("FIRST"), pick("S", start + 4000)),
            (station("UNMAPPED"), pick("P", start + 300)),
            (station("FIRST"), pick("P", start + 1000)),
        ];
        let live = &mut model.seismic.live_stations;
        live.follow.observe(&"old".into(), start - 60000, &picks);
        live.follow.observe(&quake, start, &picks);
        assert!(live.follow.next(&live.stations).is_none());
        for (code, lat) in [("LATE", 10.), ("FIRST", 39.)] {
            live.stations
                .entry(station(code))
                .or_default()
                .epochs
                .push(Epoch {
                    point: GeoPoint { lat, lon: -74. },
                    start_ms: 0,
                    end_ms: i64::MAX,
                });
        }
        // A revised origin arriving before the station is visited is the same earthquake.
        live.follow.observe(&quake, start + 1, &picks);
        model.tick_event_follow(302.);
        assert!(model.follow_recaps.active.is_none());
        let target = model.selected_event().unwrap();
        assert_eq!(target.id, "station-arrival-test-quake");
        assert_eq!(target.location.lat, 39.);
        let req = model.seismic.seismogram.request.as_ref().unwrap();
        assert_eq!(req.station, station("FIRST"));
        assert_eq!(req.arrival.as_ref().unwrap().0.time_ms, start + 1000);
        assert!(model.seismic.seismogram.open);
        model.seismic.seismogram.open = false;
        let live = &mut model.seismic.live_stations;
        live.follow.observe(&quake, start, &picks);
        assert!(live.follow.next(&live.stations).is_none());
        model.tick_event_follow(313.);
        assert!(model.globe_view.zoom > 1.);
        assert!(!model.seismic.seismogram.open);
        model.tick_event_follow(313. + f64::from(model.event_follow.dwell_seconds()));
        assert_eq!(model.selected_event_id.as_deref(), Some("factal-live"));
        model.stop_event_follow();
        assert!(!model.event_follow.enabled());
    }

    #[test]
    fn replay_uses_station_epochs_and_absolute_pick_times_across_earthquakes() {
        let mut day = StationDay::new(DAY, DAY * 2);
        let start = day.from * 1000;
        let end = day.to * 1000;
        let point = GeoPoint {
            lat: 40.0,
            lon: -74.0,
        };
        for id in ["ABC", "QUIET"] {
            day.stations
                .entry(station(id))
                .or_default()
                .epochs
                .push(Epoch {
                    point,
                    start_ms: start,
                    end_ms: end - 1,
                });
        }
        day.add_picks(
            "later".into(),
            start + 2000,
            BTreeMap::from([
                (
                    station("ABC"),
                    vec![pick("S", start + 3000), pick("S", start + 3000)],
                ),
                (station("MISSING"), vec![pick("P", start + 2500)]),
            ]),
        );
        // Yesterday's earthquake contributes today's arrival, excluding its earlier pick.
        day.add_picks(
            "earlier".into(),
            start - 5000,
            BTreeMap::from([(
                station("ABC"),
                vec![pick("P", start - 1), pick("Pg", start + 7)],
            )]),
        );
        day.add_picks(
            "tomorrow".into(),
            end,
            BTreeMap::from([(station("ABC"), vec![pick("P", end - 1), pick("S", end)])]),
        );
        let abc = &day.stations[&station("ABC")];
        assert!(abc.location(start - 1).is_none());
        assert!(abc.last_arrival(start).is_none());
        assert_eq!(abc.last_arrival(end).unwrap().event.as_ref(), "tomorrow");
        assert_eq!(
            abc.last_arrival(start + 3000).unwrap().pick.family(),
            Phase::S
        );
        assert_eq!(
            abc.last_arrival(start + 7).unwrap().event.as_ref(),
            "earlier"
        );
        let quiet = &day.stations[&station("QUIET")];
        assert!(quiet.location(start).is_some());
        assert!(quiet.last_arrival(end).is_none());
        assert_eq!((day.mapped_picks, day.unmapped_picks), (3, 1));
        assert_eq!(day.bins.iter().sum::<u32>(), 3);
    }

    #[test]
    fn station_day_keeps_its_clock_without_local_events_and_exits_through_replay_controls() {
        use crate::model::{AppModel, ReplayState};
        use egui::{CentralPanel, Context, Event, PointerButton, RawInput, Shape};
        let ctx = Context::default();
        let mut model = AppModel::new();
        model.replay_mode = true;
        model.station_day = Some(StationDay::new(DAY, DAY * 2));
        let mut clock = ReplayState::new(Vec::new(), DAY, DAY * 2, 120.0);
        clock.pause();
        clock.seek(0.5);
        model.replay_state = Some(clock);

        // A completed background Factal backfill must leave the station timeline alone.
        model.rebuild_replay_state();
        let frame = |model: &mut AppModel, events| {
            ctx.run(
                RawInput {
                    events,
                    screen_rect: Some(egui::Rect::from_min_max(
                        egui::Pos2::ZERO,
                        egui::pos2(900., 600.),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    CentralPanel::default().show(ctx, |ui| {
                        ui.expand_to_include_rect(ui.max_rect());
                        crate::panels::render_replay_controls(ui, model);
                    });
                },
            )
        };
        frame(&mut model, vec![]); // egui measures the floating replay panel on its first pass.
        let text_position = |out: &egui::FullOutput, label: &str| {
            out.shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    Shape::Text(t) if t.galley.text() == label => {
                        Some(t.pos + t.galley.size() * 0.5)
                    }
                    _ => None,
                })
                .unwrap_or_else(|| panic!("Missing {label}"))
        };
        let click = |model: &mut AppModel, pos| {
            for pressed in [true, false] {
                frame(
                    model,
                    vec![
                        Event::PointerMoved(pos),
                        Event::PointerButton {
                            pos,
                            button: PointerButton::Primary,
                            pressed,
                            modifiers: Default::default(),
                        },
                    ],
                );
            }
        };
        let output = frame(&mut model, vec![]);
        text_position(&output, "1970-01-02 12:00:00 UTC");
        click(&mut model, text_position(&output, "▶ Play"));
        assert!(!model.replay_state.as_ref().unwrap().is_paused());
        let output = frame(&mut model, vec![]);
        click(&mut model, text_position(&output, "Events"));
        assert!(model.replay_mode);
        assert!(model.station_day.is_none());
    }

    #[test]
    #[ignore = "downloads historical USGS and FDSN data"]
    fn new_jersey_station_day_from_public_archives() {
        crate::event_store::open();
        let date = NaiveDate::from_ymd_opt(2024, 4, 5).unwrap();
        let mut day = StationDay::open(
            date,
            crate::event_store::now_unix(),
            &egui::Context::default(),
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(600);
        while day.loading() {
            assert!(std::time::Instant::now() < deadline, "archive timeout");
            day.poll();
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert!(day.stations.len() > 1000);
        let nj = day
            .stations
            .values()
            .filter(|station| {
                station
                    .arrivals
                    .iter()
                    .any(|p| p.event.as_ref() == "us7000ma74")
            })
            .count();
        eprintln!(
            "{} stations, {} mapped picks, {} with a New Jersey arrival; {} of {:?} quakes checked ({} unavailable, {} unmapped picks); {:?} {:?}",
            day.stations.len(),
            day.mapped_picks,
            nj,
            day.processed,
            day.catalog_size,
            day.failed,
            day.unmapped_picks,
            day.warnings,
            day.last_error
        );
        assert!(nj > 4);
    }
}
