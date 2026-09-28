//! Feed identity and smooth camera motion for the live-event idle mode.
use super::{EventRecord, GeoPoint, GlobeViewState};
use std::collections::HashSet;

const FLIGHT_SECONDS: f64 = 10.0;
const ORBIT_RATE: f64 = 0.035;
// Latitude, unwrapped longitude, log zoom, local yaw, local pitch.
type Pose = [f64; 5];

#[derive(Default)]
pub struct EventFollow {
    enabled: bool,
    seen: HashSet<String>,
    pending: Option<EventRecord>,
    target: Option<EventRecord>,
    flight: Option<Flight>,
    velocity: Pose,
}

struct Flight {
    started: f64,
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

    /// Poll updates are not arrivals. Remember identities even while disabled,
    /// and pick the newest geolocated Factal record if a poll contains a burst.
    pub fn observe(&mut self, events: &[EventRecord]) {
        let fresh = newest(events.iter().filter(|e| {
            let new = self.seen.insert(e.id.clone());
            new && valid(e)
        }))
        .cloned();
        if self.enabled {
            if let Some(event) = fresh {
                self.pending = Some(event);
            }
            // Preserve the followed brief when the item ages out of the live page.
            if let Some(target) = &mut self.target {
                if let Some(updated) = events.iter().find(|e| e.id == target.id) {
                    *target = updated.clone();
                }
            }
        }
    }

    pub fn enable(&mut self, events: &[EventRecord]) {
        self.enabled = true;
        self.seen.extend(events.iter().map(|e| e.id.clone()));
        self.pending = newest(events.iter().filter(|e| valid(e))).cloned();
    }

    pub fn stop(&mut self) {
        self.enabled = false;
        self.pending = None;
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
        let selected = self.pending.take().map(|event| {
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

fn valid(event: &EventRecord) -> bool {
    event.factal_brief.is_some()
        && event.location.lat.is_finite()
        && event.location.lon.is_finite()
        && event.location.lat.abs() <= 90.0
        && event.location.lon.abs() <= 180.0
}

fn newest<'a>(events: impl Iterator<Item = &'a EventRecord>) -> Option<&'a EventRecord> {
    events.max_by(|a, b| timestamp(a).cmp(&timestamp(b)).then(a.id.cmp(&b.id)))
}

fn timestamp(event: &EventRecord) -> i64 {
    event
        .factal_brief
        .as_ref()
        .and_then(|b| b.occurred_at_raw.as_deref())
        .filter(|s| s.is_ascii() && s.len() >= 10)
        .and_then(crate::event_store::parse_iso_to_unix)
        .unwrap_or(0)
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
            if local { start[3] + 0.25 } else { start[3] },
            if local { 0.90 } else { start[4] },
        ];
        let mut final_velocity = [0.0; 5];
        if local {
            final_velocity[3] = ORBIT_RATE;
        } else {
            end[0] += 0.8;
            final_velocity[1] = 0.8 / lat.to_radians().cos().max(0.15) * ORBIT_RATE;
        }
        let excursion = (distance / if local { 8.0 } else { 70.0 }).clamp(0.0, 1.0);
        let near_zoom = start[2].min(end[2]);
        let wide_zoom =
            near_zoom + ((if local { 1.0f64 } else { 0.8f64 }).ln() - near_zoom) * excursion;
        Self {
            started: now,
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
        if elapsed >= FLIGHT_SECONDS {
            let t = elapsed - FLIGHT_SECONDS;
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
                FLIGHT_SECONDS,
            );
        }
        if self.zoom_out {
            (p[2], v[2]) = if elapsed < 3.0 {
                curve(
                    self.start[2],
                    self.wide_zoom,
                    self.initial_velocity[2],
                    0.0,
                    elapsed,
                    3.0,
                )
            } else {
                curve(self.wide_zoom, self.end[2], 0.0, 0.0, elapsed - 3.0, 7.0)
            };
        }
        (p, v)
    }
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
