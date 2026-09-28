//! Ranked event touring and smooth camera motion for the live-event idle mode.
use super::{EventRecord, GeoPoint, GlobeViewState};
#[path = "event_tour.rs"]
mod event_tour;
use event_tour::EventTour;

const MAX_FLIGHT_SECONDS: f64 = 10.0;
const MIN_FLIGHT_SECONDS: f64 = 0.65;
const ORBIT_SECONDS: f64 = 10.0;
const ORBIT_RATE: f64 = 0.035;
// Latitude, unwrapped longitude, log zoom, local yaw, local pitch.
type Pose = [f64; 5];

#[derive(Default)]
pub struct EventFollow {
    enabled: bool,
    tour: EventTour,
    target: Option<EventRecord>,
    flight: Option<Flight>,
    velocity: Pose,
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
}

impl EventFollow {
    pub fn enabled(&self) -> bool {
        self.enabled
    }
    pub fn target(&self) -> Option<&EventRecord> {
        self.target.as_ref()
    }
    pub fn moving(&self) -> bool {
        self.enabled && self.flight.is_some()
    }

    pub fn tour_position(&self) -> Option<(usize, usize)> {
        self.target
            .as_ref()
            .and_then(|target| self.tour.position(&target.id))
    }

    /// Each successful live payload replaces the ranked tour, including updated
    /// severity/brief metadata. Repeated payloads preserve the active stop.
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
        self.tour = EventTour::default();
        self.tour.refresh(events, None);
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
        let next = self.tour.take_pending().or_else(|| {
            let dwell_finished = self
                .flight
                .as_ref()
                .is_none_or(|flight| now >= flight.started + flight.duration + ORBIT_SECONDS);
            dwell_finished
                .then(|| self.tour.next(self.target.as_ref().map(|e| e.id.as_str())))
                .flatten()
        });
        let selected = next.map(|event| {
            if self.flight.is_none() {
                self.velocity = view_velocity(view);
            }
            self.flight = Some(Flight::new(view, self.velocity, event.location, now));
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
        [
            view.vel_local_lat as f64,
            view.vel_local_lon as f64,
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
    let center = GeoPoint {
        lat: p[0].clamp(-85.0, 85.0) as f32,
        lon: wrap(p[1]) as f32,
    };
    view.local_center = center;
    view.yaw = center.lon.to_radians() - std::f32::consts::FRAC_PI_2;
    view.pitch = center.lat.to_radians();
    if view.local_mode {
        view.local_zoom = p[2].exp().clamp(1.0, 60.0) as f32;
        view.local_yaw = p[3] as f32;
        view.local_pitch = p[4].clamp(0.02, 1.55) as f32;
    } else {
        view.zoom = p[2].exp().clamp(0.6, 50.0) as f32;
    }
}

fn wrap(lon: f64) -> f64 {
    (lon + 180.0).rem_euclid(360.0) - 180.0
}

impl Flight {
    fn new(view: &GlobeViewState, velocity: Pose, location: GeoPoint, now: f64) -> Self {
        let start = pose(view);
        let local = view.local_mode;
        let center = GeoPoint {
            lat: location.lat.clamp(-84.0, 84.0),
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
            if local { 9.5f64.ln() } else { 35.0f64.ln() },
            start[3],
            if local { 0.90 } else { start[4] },
        ];
        // Size the travel from the camera's current position to its arrival
        // pose, not from the previous event's coordinates (retargets can happen
        // anywhere along a flight or orbit).
        if !local {
            end[0] += 0.8;
        }
        let duration = flight_duration(start, end);
        if local {
            end[3] = start[3] + ORBIT_RATE * duration;
        }
        let mut final_velocity = [0.0; 5];
        if local {
            final_velocity[3] = ORBIT_RATE;
        } else {
            final_velocity[1] = 0.8 / lat.to_radians().cos().max(0.15) * ORBIT_RATE;
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
        }
    }

    fn sample(&self, now: f64) -> (Pose, Pose) {
        let elapsed = (now - self.started).max(0.0);
        if elapsed >= self.duration {
            let t = elapsed - self.duration;
            let mut p = self.end;
            let mut v = [0.0; 5];
            if self.local {
                p[3] += ORBIT_RATE * t;
                v[3] = ORBIT_RATE;
            } else {
                let phase = ORBIT_RATE * t;
                let r_lon = 0.8 / (self.orbit_center.lat as f64).to_radians().cos().max(0.15);
                p[0] = self.orbit_center.lat as f64 + 0.8 * phase.cos();
                p[1] = self.end[1] + r_lon * phase.sin();
                v[0] = -0.8 * ORBIT_RATE * phase.sin();
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
                    0.0,
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
