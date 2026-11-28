//! Live/regional-recap scheduling. Disk loading stays on a short-lived worker.
pub use super::recap_regions::RecapRegion;
use super::{ActiveBody, AppModel, EventRecord, GeoPoint, GlobeViewState, ReplayState};
use std::sync::mpsc::{self, Receiver, TryRecvError};

pub const LIVE_SECONDS: f64 = 300.0;
pub const RECAP_SECONDS: f64 = 60.0;
const FRAMING_SECONDS: f64 = 3.0;
const DAY: i64 = 86_400;
type History = Vec<(i64, EventRecord)>;

#[derive(Default)]
pub struct FollowRecaps {
    live_since: Option<f64>,
    next: usize,
    pub active: Option<RegionalRecap>,
}

pub struct RegionalRecap {
    pub region: RecapRegion,
    pub replay: ReplayState,
    pub started: f64,
    saved_view: GlobeViewState,
    pending: Option<Receiver<History>>,
    play_started: Option<f64>,
}

impl RegionalRecap {
    fn new(region: RecapRegion, now: f64, unix: i64, view: GlobeViewState) -> Self {
        let mut recap = Self::with_events(region, now, unix, view, Vec::new());
        let (tx, rx) = mpsc::channel();
        recap.pending = Some(rx);
        std::thread::spawn(move || {
            let events = crate::event_store::load_events_in_range(unix - DAY, unix);
            let _ = tx.send(events);
            crate::app::request_repaint();
        });
        recap
    }

    pub(crate) fn with_events(
        region: RecapRegion,
        now: f64,
        unix: i64,
        view: GlobeViewState,
        events: History,
    ) -> Self {
        let mut recap = Self {
            region,
            replay: ReplayState::new_paused(Vec::new(), unix - DAY, unix, RECAP_SECONDS),
            started: now,
            saved_view: view,
            pending: None,
            play_started: None,
        };
        recap.set_events(events);
        recap
    }

    fn set_events(&mut self, mut events: History) {
        events.retain(|(unix, event)| {
            *unix >= self.replay.sim_from
                && *unix <= self.replay.sim_to
                && RecapRegion::at(event.location) == Some(self.region)
        });
        events.sort_by_key(|(unix, _)| *unix);
        self.replay.events = events;
    }

    pub fn loading(&self) -> bool {
        self.pending.is_some()
    }
    pub fn framing(&self) -> bool {
        self.play_started.is_none()
    }

    /// Elapsed time within the trailing 24-hour window, not UTC time of day.
    pub fn clock(&self) -> String {
        let seconds = ((self.replay.wall_elapsed() / RECAP_SECONDS).clamp(0.0, 1.0) * DAY as f64)
            .floor()
            .min((DAY - 1) as f64) as i64;
        format!(
            "{:02}:{:02}:{:02}",
            seconds / 3600,
            seconds / 60 % 60,
            seconds % 60
        )
    }

    pub(crate) fn tick(&mut self, view: &mut GlobeViewState, now: f64) -> bool {
        if let Some(rx) = &self.pending {
            match rx.try_recv() {
                Ok(events) => {
                    self.pending = None;
                    self.set_events(events);
                }
                Err(TryRecvError::Disconnected) => {
                    self.pending = None;
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        // Smoothly widen and center before starting the one-minute clock.
        let t = ((now - self.started) / FRAMING_SECONDS).clamp(0.0, 1.0) as f32;
        let t = t * t * (3.0 - 2.0 * t);
        let origin = if self.saved_view.local_mode {
            self.saved_view.local_center
        } else {
            self.saved_view.globe_center_latlon()
        };
        let center = self.region.center();
        let lon_delta = (center.lon - origin.lon + 180.0).rem_euclid(360.0) - 180.0;
        view.local_mode = false;
        view.focus_on(GeoPoint {
            lat: origin.lat + (center.lat - origin.lat) * t,
            lon: origin.lon + lon_delta * t,
        });
        let start_zoom = if self.saved_view.local_mode {
            1.0
        } else {
            self.saved_view.zoom
        };
        view.zoom = (start_zoom.ln() * (1.0 - t) + self.region.zoom().ln() * t).exp();
        view.stop_motion();
        if t >= 1.0 && !self.loading() {
            let started = *self.play_started.get_or_insert(now);
            self.replay.advance_to(now - started);
        }
        // Give the final 23:59:59 and reports at the window endpoint a visible frame.
        self.play_started
            .is_some_and(|started| now - started >= RECAP_SECONDS + 0.5)
    }

    fn restore(&self, view: &mut GlobeViewState) {
        if self.saved_view.local_mode {
            *view = self.saved_view;
        }
        view.stop_motion();
    }
}

impl AppModel {
    pub fn toggle_event_follow(&mut self) {
        if self.event_follow.enabled() {
            self.stop_event_follow();
        } else if self.active_body == ActiveBody::Earth {
            self.station_day = None;
            self.replay_mode = false;
            self.replay_state = None;
            self.follow_recaps = FollowRecaps::default();
            self.convergence.reset_tour_history();
            self.refresh_convergence(0.0, true);
            self.globe_view.stop_motion();
            self.event_follow.enable(&self.events);
            self.seismic
                .live_stations
                .follow
                .set_enabled(true, chrono::Utc::now().timestamp_millis());
            // Warm the one-day cache once per follow session, independently of live polling.
            let now = crate::event_store::now_unix();
            if self.has_factal_api_key()
                && !crate::factal_stream::is_history_fetching()
                && crate::event_store::oldest_event_unix().is_none_or(|oldest| oldest > now - DAY)
            {
                crate::factal_stream::trigger_history_fetch(self.factal_api_key.clone(), 1);
            }
        }
    }

    pub fn stop_event_follow(&mut self) {
        if let Some(recap) = self.follow_recaps.active.take() {
            recap.restore(&mut self.globe_view);
        }
        self.follow_recaps = FollowRecaps::default();
        if self.event_follow.enabled() {
            self.event_follow.stop();
            self.seismic.live_stations.follow.set_enabled(false, 0);
            self.globe_view.stop_motion();
        }
    }

    pub fn active_replay(&self) -> Option<&ReplayState> {
        self.follow_recaps
            .active
            .as_ref()
            .map(|recap| &recap.replay)
            .or(self.replay_state.as_ref().filter(|_| self.replay_mode))
    }

    /// Atlas sunlight follows the active regional recap or synchronized replay playhead,
    /// then resumes the app stellar clock.
    pub fn atlas_light_jd(&self) -> f64 {
        if let Some(recap) = self.follow_recaps.active.as_ref() {
            let replay = &recap.replay;
            return crate::stellar_time::unix_to_jd(replay.unix_at_elapsed(replay.wall_elapsed()));
        }
        if self.master_clock.sync_subsystems
            && self.replay_mode
            && let Some(replay) = self.replay_state.as_ref()
        {
            return crate::stellar_time::unix_to_jd(replay.unix_at_elapsed(replay.wall_elapsed()));
        }
        self.stellar_jd
    }

    pub fn refresh_convergence(&mut self, now_anim: f64, force: bool) {
        let now_unix = crate::event_store::now_unix();
        self.convergence.refresh(
            now_anim,
            now_unix,
            force,
            &self.events,
            &self.cameras,
            &self.active_fires,
            self.fire_snapshot_revision(),
            &self.seismic.live_stations,
            &self.flights,
            &self.tracks,
        );
    }

    pub fn tick_event_follow(&mut self, now: f64) {
        if self.active_body != ActiveBody::Earth || self.replay_mode {
            self.stop_event_follow();
            return;
        }
        if !self.event_follow.enabled() {
            return;
        }
        self.refresh_convergence(now, false);
        let director = self.follow_director.normalized();

        let live = &mut self.seismic.live_stations;
        let station = if self.event_follow.at_station(now) {
            None
        } else {
            live.follow.next(&live.stations).filter(|stop| {
                if !director.smart_director {
                    return true;
                }
                // Gate immediate preemption only when we have a known USGS magnitude below threshold.
                let usgs_id = format!("{}{}", super::USGS_EVENT_PREFIX, stop.event);
                match self
                    .events
                    .iter()
                    .find(|e| e.id == usgs_id)
                    .and_then(|e| super::convergence::parse_quake_magnitude(&e.title))
                {
                    Some(mag) => mag >= director.min_quake_interrupt_mag,
                    None => true,
                }
            })
        };
        if station.is_some() {
            if let Some(recap) = self.follow_recaps.active.take() {
                recap.restore(&mut self.globe_view);
            }
            self.follow_recaps.live_since = Some(now);
        }
        if let Some(recap) = &mut self.follow_recaps.active {
            if !recap.tick(&mut self.globe_view, now) {
                return;
            }
            let recap = self.follow_recaps.active.take().unwrap();
            recap.restore(&mut self.globe_view);
            self.follow_recaps.live_since = Some(now);
        } else {
            let since = *self.follow_recaps.live_since.get_or_insert(now);
            if now - since >= LIVE_SECONDS && !self.event_follow.at_station(now) {
                let mut chosen = self.follow_recaps.next;
                if director.skip_empty_recap_regions && !self.events.is_empty() {
                    for offset in 0..RecapRegion::ALL.len() {
                        let idx = (self.follow_recaps.next + offset) % RecapRegion::ALL.len();
                        let candidate = RecapRegion::ALL[idx];
                        if self
                            .events
                            .iter()
                            .any(|e| RecapRegion::at(e.location) == Some(candidate))
                        {
                            chosen = idx;
                            break;
                        }
                    }
                }
                let region = RecapRegion::ALL[chosen];
                self.follow_recaps.next = (chosen + 1) % RecapRegion::ALL.len();
                self.event_follow.release_camera();
                self.follow_recaps.active = Some(RegionalRecap::new(
                    region,
                    now,
                    crate::event_store::now_unix(),
                    self.globe_view,
                ));
                self.follow_recaps
                    .active
                    .as_mut()
                    .unwrap()
                    .tick(&mut self.globe_view, now);
                return;
            }
        }
        let quake_stop = station.map(|stop| {
            let time = chrono::DateTime::from_timestamp_millis(stop.pick.time_ms).unwrap();
            let event = EventRecord {
                id: format!("station-arrival-{}", stop.event),
                title: format!("Earthquake · {}", stop.station),
                summary: format!(
                    "Earliest mapped {} arrival · USGS {}",
                    stop.pick.phase, stop.event
                ),
                occurred_at: time.to_rfc3339(),
                severity: super::EventSeverity::Unrated,
                location_name: stop.station.to_string(),
                location: stop.point,
                source: "USGS phase data".into(),
                factal_brief: None,
            };
            self.seismic
                .seismogram
                .arrival(stop.station, stop.pick, stop.event.to_string());
            event
        });
        let is_quake = quake_stop.is_some();
        let selected = if is_quake {
            let directive = if director.adaptive_framing {
                super::convergence::StopDirective {
                    globe_zoom: 24.0,
                    local_zoom: 7.5,
                    local_pitch: 0.82,
                    orbit_radius_deg: 1.0,
                    dwell_multiplier: 1.0,
                    dwell_zoom_drift: 0.002,
                }
            } else {
                super::convergence::StopDirective::default()
            };
            self.event_follow.tick_with_director(
                &mut self.globe_view,
                now,
                quake_stop.map(|e| (e, directive)),
                None,
            )
        } else if director.smart_director {
            let current_id = self.event_follow.target().map(|e| e.id.clone());
            let current_loc = self.event_follow.target().map(|e| e.location);
            let breaking = if !self.event_follow.at_station(now) {
                self.event_follow.dwell_elapsed(now).and_then(|elapsed| {
                    self.convergence.breaking_interrupt(
                        current_id.as_deref(),
                        elapsed,
                        10.0,
                        director.adaptive_framing,
                    )
                })
            } else {
                None
            };
            let next_stop = if breaking.is_none()
                && !self.event_follow.at_station(now)
                && self.event_follow.dwell_finished(now)
            {
                self.convergence.select_next_stop(
                    current_id.as_deref(),
                    current_loc,
                    director.adaptive_framing,
                )
            } else {
                None
            };
            self.event_follow
                .tick_with_director(&mut self.globe_view, now, breaking, next_stop)
        } else {
            self.event_follow.tick(&mut self.globe_view, now)
        };
        if let Some(id) = selected {
            self.focused_city_id = None;
            self.selected_event_id = Some(id);
            self.selected_camera_id = self.nearby_cameras(250.0).first().map(|c| c.id.clone());
            self.factal_brief_open = !is_quake;
        }
    }
}

#[cfg(test)]
#[path = "follow_recap_tests.rs"]
mod tests;
