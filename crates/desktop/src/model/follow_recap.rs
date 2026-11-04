//! Live/continental-recap scheduling. Disk loading stays on a short-lived worker.
pub use super::continents::Continent;
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
    pub active: Option<ContinentalRecap>,
}

pub struct ContinentalRecap {
    pub continent: Continent,
    pub replay: ReplayState,
    pub started: f64,
    saved_view: GlobeViewState,
    pending: Option<Receiver<History>>,
    play_started: Option<f64>,
}

impl ContinentalRecap {
    fn new(continent: Continent, now: f64, unix: i64, view: GlobeViewState) -> Self {
        let mut recap = Self::with_events(continent, now, unix, view, Vec::new());
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
        continent: Continent,
        now: f64,
        unix: i64,
        view: GlobeViewState,
        events: History,
    ) -> Self {
        let mut replay = ReplayState::new(Vec::new(), unix - DAY, unix, RECAP_SECONDS);
        replay.pause();
        let mut recap = Self {
            continent,
            replay,
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
                && Continent::at(event.location) == Some(self.continent)
        });
        events.sort_by_key(|(unix, _)| *unix);
        self.replay = ReplayState::new(
            events,
            self.replay.sim_from,
            self.replay.sim_to,
            RECAP_SECONDS,
        );
        self.replay.pause();
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
        let center = self.continent.center();
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
        view.zoom = (start_zoom.ln() * (1.0 - t) + 1.15_f32.ln() * t).exp();
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
            self.replay_mode = false;
            self.replay_state = None;
            self.follow_recaps = FollowRecaps::default();
            self.globe_view.stop_motion();
            self.event_follow.enable(&self.events);
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

    pub fn tick_event_follow(&mut self, now: f64) {
        if self.active_body != ActiveBody::Earth || self.replay_mode {
            self.stop_event_follow();
            return;
        }
        if !self.event_follow.enabled() {
            return;
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
            if now - since >= LIVE_SECONDS {
                let continent = Continent::ALL[self.follow_recaps.next];
                self.follow_recaps.next = (self.follow_recaps.next + 1) % Continent::ALL.len();
                self.event_follow.release_camera();
                self.follow_recaps.active = Some(ContinentalRecap::new(
                    continent,
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
        if let Some(id) = self.event_follow.tick(&mut self.globe_view, now) {
            self.focused_city_id = None;
            self.selected_event_id = Some(id);
            self.selected_camera_id = self.nearby_cameras(250.0).first().map(|c| c.id.clone());
            self.factal_brief_open = true;
        }
    }
}

#[cfg(test)]
#[path = "follow_recap_tests.rs"]
mod tests;
