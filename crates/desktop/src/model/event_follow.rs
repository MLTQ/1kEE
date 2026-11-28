//! Ranked event touring and smooth camera motion for the live-event idle mode.
use super::convergence::StopDirective;
use super::{EventRecord, GeoPoint, GlobeViewState};
use crate::settings_store::{
    DEFAULT_EVENT_FOLLOW_DWELL_SECONDS, normalize_event_follow_dwell_seconds,
};
#[path = "event_tour.rs"]
mod event_tour;
use event_tour::EventTour;

const MAX_FLIGHT_SECONDS: f64 = 10.0;
const MIN_FLIGHT_SECONDS: f64 = 0.65;
const ORBIT_RATE: f64 = 0.035;
const LABEL_FADE_IN_SECONDS: f64 = 0.45;
const LABEL_FADE_OUT_SECONDS: f64 = 0.25;
const MAX_DWELL_LOG_ZOOM_DRIFT: f64 = 0.18;
// Latitude, unwrapped longitude, log zoom, local yaw, local pitch.
type Pose = [f64; 5];

pub struct EventFollow {
    enabled: bool,
    tour: EventTour,
    target: Option<EventRecord>,
    flight: Option<Flight>,
    velocity: Pose,
    dwell_seconds: u32,
}

impl Default for EventFollow {
    fn default() -> Self {
        Self::with_dwell_seconds(DEFAULT_EVENT_FOLLOW_DWELL_SECONDS)
    }
}

struct Flight {
    started: f64,
    duration: f64,
    local: bool,
    start: Pose,
    end: Pose,
    initial_velocity: Pose,
    final_velocity: Pose,
    wide_zoom: f64,
    zoom_out: bool,
    orbit_center: GeoPoint,
    departure_label_opacity: f32,
    directive: StopDirective,
}

impl EventFollow {
    pub fn with_dwell_seconds(seconds: u32) -> Self {
        Self {
            enabled: false,
            tour: EventTour::default(),
            target: None,
            flight: None,
            velocity: [0.0; 5],
            dwell_seconds: normalize_event_follow_dwell_seconds(seconds),
        }
    }

    pub fn dwell_seconds(&self) -> u32 {
        self.dwell_seconds
    }

    /// Applies to the current stop without restarting its arrival clock or flight.
    pub fn set_dwell_seconds(&mut self, seconds: u32) {
        self.dwell_seconds = normalize_event_follow_dwell_seconds(seconds);
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }
    pub fn target(&self) -> Option<&EventRecord> {
        self.target.as_ref()
    }
    pub fn moving(&self) -> bool {
        self.enabled && self.flight.is_some()
    }

    /// The committed landing pose is stable even when a feed refresh moves a story.
    pub(crate) fn local_landing(&self, now: f64) -> Option<(GeoPoint, f32)> {
        let flight = self.flight.as_ref()?;
        (self.enabled && flight.local && now < flight.started + flight.duration)
            .then(|| (flight.orbit_center, flight.end[2].exp() as f32))
    }

    /// Regional cards appear only after arrival at the flight's destination
    /// zoom. Capture their current alpha on departure to fade out continuously,
    /// including when a new event interrupts an unfinished fade.
    pub fn event_label_opacity(&self, now: f64) -> f32 {
        let Some(flight) = self.flight.as_ref().filter(|_| self.enabled) else {
            return 0.0;
        };
        let elapsed = (now - flight.started).max(0.0);
        if elapsed >= flight.duration {
            curve(
                0.0,
                1.0,
                0.0,
                0.0,
                elapsed - flight.duration,
                LABEL_FADE_IN_SECONDS,
            )
            .0 as f32
        } else {
            curve(
                flight.departure_label_opacity as f64,
                0.0,
                0.0,
                0.0,
                elapsed,
                LABEL_FADE_OUT_SECONDS,
            )
            .0 as f32
        }
    }

    pub fn tour_position(&self) -> Option<(usize, usize)> {
        self.target
            .as_ref()
            .and_then(|target| self.tour.position(&target.id))
    }

    /// Refresh all eligible stories and their severity/brief metadata while
    /// preserving visit history and the current flight/dwell.
    pub fn observe(&mut self, events: &[EventRecord]) {
        if self.enabled {
            self.tour
                .refresh(events, self.target.as_ref().map(|e| e.id.as_str()));
            if let Some(target) = &mut self.target
                && let Some(updated) = events.iter().find(|e| e.id == target.id)
            {
                *target = updated.clone();
            }
        }
    }

    pub fn enable(&mut self, events: &[EventRecord]) {
        self.enabled = true;
        self.target = None;
        self.tour = EventTour::default();
        self.tour.refresh(events, None);
    }

    /// Release the current flight while retaining weighted tour history and dwell.
    pub fn release_camera(&mut self) {
        self.flight = None;
        self.velocity = [0.0; 5];
    }

    pub fn stop(&mut self) {
        self.enabled = false;
        self.tour = EventTour::default();
        self.flight = None;
        self.velocity = [0.0; 5];
    }

    /// Returns a newly followed event ID so the model can select it without
    /// using the ordinary click-to-focus snap. Uses absolute animation time.
    pub fn tick(&mut self, view: &mut GlobeViewState, now: f64) -> Option<String> {
        self.tick_with_stop(view, now, None)
    }

    pub(super) fn at_station(&self, now: f64) -> bool {
        self.target
            .as_ref()
            .is_some_and(|e| e.id.starts_with("station-arrival-"))
            && self
                .flight
                .as_ref()
                .is_some_and(|f| now < f.started + f.duration + self.effective_dwell_seconds(f))
    }

    /// Seconds elapsed since arriving at the current stop (`None` while in transit or idle).
    pub(super) fn dwell_elapsed(&self, now: f64) -> Option<f64> {
        let flight = self.flight.as_ref()?;
        let elapsed = now - (flight.started + flight.duration);
        (elapsed >= 0.0).then_some(elapsed)
    }

    /// True if there is no active flight or the current stop's dwell period has elapsed.
    pub(super) fn dwell_finished(&self, now: f64) -> bool {
        self.flight.as_ref().is_none_or(|flight| {
            now >= flight.started + flight.duration + self.effective_dwell_seconds(flight)
        })
    }

    fn effective_dwell_seconds(&self, flight: &Flight) -> f64 {
        (f64::from(self.dwell_seconds) * flight.directive.dwell_multiplier).max(5.0)
    }

    pub(super) fn tick_with_stop(
        &mut self,
        view: &mut GlobeViewState,
        now: f64,
        station: Option<EventRecord>,
    ) -> Option<String> {
        self.tick_with_director(view, now, station.map(|e| (e, StopDirective::default())), None)
    }

    /// Advance the Follow Events camera with an optional preempting stop (e.g. quake arrival
    /// or breaking convergence) and an optional director callback invoked when the current
    /// dwell completes. Falls back to the baseline `EventTour` when the director returns `None`.
    pub(super) fn tick_with_director(
        &mut self,
        view: &mut GlobeViewState,
        now: f64,
        preempt: Option<(EventRecord, StopDirective)>,
        director_next: Option<(EventRecord, StopDirective)>,
    ) -> Option<String> {
        if !self.enabled {
            return None;
        }
        // Evaluate the old trajectory first so retargets have position AND
        // velocity continuity even when the feed arrives between frames.
        if let Some(flight) = &self.flight {
            let (pose, velocity) = flight.sample(now);
            apply(view, pose);
            self.velocity = velocity;
        }
        let next = preempt.or_else(|| {
            if self.at_station(now) {
                return None;
            }
            if let Some(pending) = self.tour.take_pending() {
                return Some((pending, StopDirective::default()));
            }
            if self.dwell_finished(now) {
                director_next.or_else(|| {
                    self.tour
                        .next(self.target.as_ref().map(|e| e.id.as_str()))
                        .map(|e| (e, StopDirective::default()))
                })
            } else {
                None
            }
        });
        let selected = next.map(|(event, directive)| {
            if self.flight.is_none() {
                self.velocity = view_velocity(view);
            }
            let mut flight =
                Flight::new_with_directive(view, self.velocity, event.location, now, directive);
            flight.departure_label_opacity = self.event_label_opacity(now);
            self.flight = Some(flight);
            let id = event.id.clone();
            self.target = Some(event);
            id
        });
        if self.flight.is_some() {
            view.stop_motion();
        }
        selected
    }
}

fn pose(view: &GlobeViewState) -> Pose {
    let center = if view.local_mode {
        view.local_center
    } else {
        view.globe_center_latlon()
    };
    [
        center.lat as f64,
        center.lon as f64,
        (if view.local_mode {
            view.local_zoom
        } else {
            view.zoom
        })
        .ln() as f64,
        view.local_yaw as f64,
        view.local_pitch as f64,
    ]
}

fn view_velocity(view: &GlobeViewState) -> Pose {
    if view.local_mode {
        let center = view.local_center;
        let pole = super::polar::pole_for(center.lat);
        // Estimate angular motion over one second in the local ground chart.
        // The instantaneous longitude derivative is undefined at the pole.
        let (lat_rate, lon_rate) = if pole == 0.0 {
            (view.vel_local_lat, view.vel_local_lon)
        } else {
            let next = super::polar::location(
                pole, center, egui::vec2(view.vel_local_lon, view.vel_local_lat));
            (next.lat - center.lat, next.lon - center.lon)
        };
        [
            lat_rate as f64,
            lon_rate as f64,
            0.0,
            view.vel_local_yaw as f64,
            view.vel_local_pitch as f64,
        ]
    } else {
        [
            view.vel_pitch.to_degrees() as f64,
            view.vel_yaw.to_degrees() as f64,
            0.0,
            0.0,
            0.0,
        ]
    }
}

fn apply(view: &mut GlobeViewState, p: Pose) {
    let lat = if view.local_mode {
        p[0].clamp(-90.0, 90.0)
    } else {
        p[0].clamp(-85.0, 85.0)
    };
    let lon = wrap(p[1]);
    let center = GeoPoint {
        lat: lat as f32,
        lon: lon as f32,
    };
    view.target_lat = lat;
    view.target_lon = lon;
    view.local_center = center;
    view.yaw = center.lon.to_radians() - std::f32::consts::FRAC_PI_2;
    view.pitch = center.lat.to_radians();
    if view.local_mode {
        view.local_zoom = p[2].exp().clamp(1.0, 60.0) as f32;
        view.local_yaw = p[3] as f32;
        view.local_pitch = (p[4] as f32).clamp(0.0, super::LOCAL_MAX_TILT);
        view.heading = view.local_yaw;
        view.tilt = view.local_pitch;
        view.range_m = GlobeViewState::range_for_legacy_local_zoom(view.local_zoom);
    } else {
        view.zoom = p[2].exp().clamp(0.6, 50.0) as f32;
        view.range_m = GlobeViewState::range_for_legacy_globe_zoom(
            view.zoom,
            super::ActiveBody::Earth.radius_m(),
        );
    }
}

fn wrap(lon: f64) -> f64 {
    (lon + 180.0).rem_euclid(360.0) - 180.0
}

impl Flight {
    #[cfg(test)]
    fn new(view: &GlobeViewState, velocity: Pose, location: GeoPoint, now: f64) -> Self {
        Self::new_with_directive(view, velocity, location, now, StopDirective::default())
    }

    fn new_with_directive(
        view: &GlobeViewState,
        velocity: Pose,
        location: GeoPoint,
        now: f64,
        mut directive: StopDirective,
    ) -> Self {
        directive.globe_zoom = directive.globe_zoom.clamp(1.0, 50.0);
        directive.local_zoom = directive.local_zoom.clamp(1.0, 60.0);
        directive.local_pitch = directive.local_pitch.clamp(0.0, super::LOCAL_MAX_TILT as f64);
        directive.orbit_radius_deg = directive.orbit_radius_deg.clamp(0.2, 2.5);
        directive.dwell_multiplier = directive.dwell_multiplier.clamp(0.5, 2.0);
        directive.dwell_zoom_drift = directive.dwell_zoom_drift.clamp(-0.02, 0.02);

        let start = pose(view);
        let local = view.local_mode;
        let center = GeoPoint {
            lat: if local {
                location.lat
            } else {
                location.lat.clamp(-84.0, 84.0)
            },
            lon: location.lon,
        };
        let lat = center.lat as f64;
        let lon = start[1] + wrap(center.lon as f64 - start[1]);
        let distance = ((lat - start[0]).powi(2)
            + ((lon - start[1]) * ((lat + start[0]) * 0.5).to_radians().cos()).powi(2))
        .sqrt();
        let mut end = [
            lat,
            lon,
            if local {
                directive.local_zoom.ln()
            } else {
                directive.globe_zoom.ln()
            },
            start[3],
            if local {
                directive.local_pitch
            } else {
                start[4]
            },
        ];
        // Size the travel from the camera's current position to its arrival
        // pose, not from the previous event's coordinates (retargets can happen
        // anywhere along a flight or orbit).
        if !local {
            end[0] += directive.orbit_radius_deg;
        }
        let duration = flight_duration(start, end);
        if local {
            end[3] = start[3] + ORBIT_RATE * duration;
        }
        let mut final_velocity = [0.0; 5];
        final_velocity[2] = directive.dwell_zoom_drift;
        if local {
            final_velocity[3] = ORBIT_RATE;
        } else {
            final_velocity[1] =
                directive.orbit_radius_deg / lat.to_radians().cos().max(0.15) * ORBIT_RATE;
        }
        let excursion = (distance / if local { 8.0 } else { 70.0 }).clamp(0.0, 1.0);
        let near_zoom = start[2].min(end[2]);
        let wide_zoom =
            near_zoom + ((if local { 1.0f64 } else { 0.8f64 }).ln() - near_zoom) * excursion;
        Self {
            started: now,
            duration,
            local,
            start,
            end,
            initial_velocity: velocity,
            final_velocity,
            wide_zoom,
            zoom_out: excursion > 0.05,
            orbit_center: center,
            departure_label_opacity: 0.0,
            directive,
        }
    }

    fn sample(&self, now: f64) -> (Pose, Pose) {
        let elapsed = (now - self.started).max(0.0);
        if elapsed >= self.duration {
            let t = elapsed - self.duration;
            let mut p = self.end;
            let mut v = [0.0; 5];
            if self.directive.dwell_zoom_drift != 0.0 {
                // Smooth C¹ bounded zoom push: derivative at t=0 equals dwell_zoom_drift.
                let u = self.directive.dwell_zoom_drift * t / MAX_DWELL_LOG_ZOOM_DRIFT;
                let th = u.tanh();
                p[2] = self.end[2] + MAX_DWELL_LOG_ZOOM_DRIFT * th;
                v[2] = self.directive.dwell_zoom_drift * (1.0 - th * th);
            }
            if self.local {
                p[3] += ORBIT_RATE * t;
                v[3] = ORBIT_RATE;
            } else {
                let r = self.directive.orbit_radius_deg;
                let phase = ORBIT_RATE * t;
                let r_lon = r / (self.orbit_center.lat as f64).to_radians().cos().max(0.15);
                p[0] = self.orbit_center.lat as f64 + r * phase.cos();
                p[1] = self.end[1] + r_lon * phase.sin();
                v[0] = -r * ORBIT_RATE * phase.sin();
                v[1] = r_lon * ORBIT_RATE * phase.cos();
            }
            return (p, v);
        }
        let mut p = [0.0; 5];
        let mut v = [0.0; 5];
        for i in 0..5 {
            (p[i], v[i]) = curve(
                self.start[i],
                self.end[i],
                self.initial_velocity[i],
                self.final_velocity[i],
                elapsed,
                self.duration,
            );
        }
        if self.zoom_out {
            let widen_seconds = self.duration * 0.3;
            (p[2], v[2]) = if elapsed < widen_seconds {
                curve(
                    self.start[2],
                    self.wide_zoom,
                    self.initial_velocity[2],
                    0.0,
                    elapsed,
                    widen_seconds,
                )
            } else {
                curve(
                    self.wide_zoom,
                    self.end[2],
                    0.0,
                    self.final_velocity[2],
                    elapsed - widen_seconds,
                    self.duration - widen_seconds,
                )
            };
        }
        (p, v)
    }
}

/// Great-circle distance makes nearby/date-line/polar hops quick. A modest
/// allowance for large zoom changes keeps an initial close-by approach smooth.
fn flight_duration(start: Pose, end: Pose) -> f64 {
    let lat0 = start[0].to_radians();
    let lat1 = end[0].to_radians();
    let dlat = lat1 - lat0;
    let dlon = wrap(end[1] - start[1]).to_radians();
    let h = ((dlat * 0.5).sin().powi(2) + lat0.cos() * lat1.cos() * (dlon * 0.5).sin().powi(2))
        .clamp(0.0, 1.0);
    let km = 2.0 * 6371.0 * h.sqrt().asin();
    let travel =
        MIN_FLIGHT_SECONDS + (MAX_FLIGHT_SECONDS - MIN_FLIGHT_SECONDS) * (km / 6000.0).sqrt();
    let zoom = (end[2] - start[2]).abs() * 0.65;
    travel
        .max(zoom)
        .clamp(MIN_FLIGHT_SECONDS, MAX_FLIGHT_SECONDS)
}

/// Quintic Hermite: prescribed endpoint velocities and zero endpoint acceleration.
fn curve(a: f64, b: f64, va: f64, vb: f64, elapsed: f64, duration: f64) -> (f64, f64) {
    let t = (elapsed / duration).clamp(0.0, 1.0);
    let t2 = t * t;
    let t3 = t2 * t;
    let t4 = t3 * t;
    let t5 = t4 * t;
    let s = 10.0 * t3 - 15.0 * t4 + 6.0 * t5;
    let h0 = t - 6.0 * t3 + 8.0 * t4 - 3.0 * t5;
    let h1 = -4.0 * t3 + 7.0 * t4 - 3.0 * t5;
    let ds = 30.0 * t2 - 60.0 * t3 + 30.0 * t4;
    let dh0 = 1.0 - 18.0 * t2 + 32.0 * t3 - 15.0 * t4;
    let dh1 = -12.0 * t2 + 28.0 * t3 - 15.0 * t4;
    (
        a + (b - a) * s + duration * (va * h0 + vb * h1),
        (b - a) * ds / duration + va * dh0 + vb * dh1,
    )
}

#[cfg(test)]
#[path = "event_follow_tests.rs"]
mod tests;
