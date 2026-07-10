//! Serializable projection of `AppModel` for the Gruve companion web view.
//!
//! The native UI owns `AppModel` on the egui thread; the HTTP server runs on its
//! own threads. Rather than share the (non-Send) model, the UI thread builds a
//! cheap `Snapshot` once per frame and stores it behind a mutex. Request handlers
//! read the snapshot and serialize the slice they need on demand — so per-frame
//! cost stays small (scalar copies + Arc clones + a handful of event/camera DTOs),
//! and the expensive part (serializing thousands of tracks) only happens when a
//! viewer actually polls.

use std::sync::Arc;

use serde::Serialize;

use crate::model::{
    ActiveBody, AppModel, CameraFeed, EventRecord, FlightTrack, GeoPoint, MovingTrack,
};

/// Hard cap on how many moving tracks / flights we serialize per request. Keeps the
/// payload bounded on a busy feed; responses report `total` and `truncated` so the
/// web view never silently implies it is showing everything.
const MAX_MOVERS: usize = 1000;

#[derive(Clone, PartialEq, Serialize)]
pub struct ViewDto {
    /// Latitude of the host's current globe centre.
    pub lat: f32,
    /// Longitude of the host's current globe centre.
    pub lon: f32,
    pub body: &'static str,
    pub theme: &'static str,
    pub local_mode: bool,
}

#[derive(Clone, PartialEq, Serialize)]
pub struct ShowFlags {
    pub events: bool,
    pub ships: bool,
    pub flights: bool,
}

#[derive(Clone, PartialEq, Serialize)]
pub struct SelectedDto {
    pub event: Option<String>,
    pub camera: Option<String>,
    pub vessel: Option<u64>,
    pub flight: Option<String>,
}

#[derive(Clone, PartialEq, Serialize)]
pub struct EventDto {
    pub id: String,
    pub title: String,
    pub summary: String,
    pub severity: &'static str,
    pub location_name: String,
    pub lat: f32,
    pub lon: f32,
    pub source: String,
    pub occurred_at: String,
}

#[derive(Clone, PartialEq, Serialize)]
pub struct CameraDto {
    pub id: String,
    pub label: String,
    pub provider: String,
    pub kind: String,
    pub lat: f32,
    pub lon: f32,
    pub status: &'static str,
    pub last_seen: String,
    // NOTE: `stream_url` is deliberately omitted. Feed connection stays on the host;
    // we publish camera *positions and reachability* to the mesh, not feed URLs.
}

#[derive(Serialize)]
pub struct TrackDto {
    pub mmsi: u64,
    pub name: String,
    pub lat: f32,
    pub lon: f32,
    pub heading: Option<f32>,
    pub speed: Option<f32>,
    pub kind: &'static str,
}

#[derive(Serialize)]
pub struct FlightDto {
    pub icao24: String,
    pub callsign: Option<String>,
    pub lat: f32,
    pub lon: f32,
    pub heading: Option<f32>,
    pub altitude: String,
    pub category: &'static str,
    pub on_ground: bool,
}

/// A capped, count-reporting list response.
#[derive(Serialize)]
struct ListResponse<T: Serialize> {
    total: usize,
    truncated: bool,
    items: Vec<T>,
}

/// Monotonic per-slice change counters. Each GET route publishes its slice's
/// generation in an `X-Gen` header and answers `304 Not Modified` when a poller's
/// `?gen=` already matches — so unchanged JSON is never re-serialized or re-sent,
/// and the web view skips the re-parse/re-render for that slice.
#[derive(Clone, Copy)]
pub struct Generations {
    pub state: u64,
    pub events: u64,
    pub cameras: u64,
    pub tracks: u64,
    pub flights: u64,
}

impl Generations {
    /// Seed every counter from wall-clock millis so a restarted host never hands
    /// out a generation an old still-open client already holds — a slice that
    /// rarely changes (cameras, say) would otherwise 304 stale data forever.
    fn fresh() -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(1);
        Generations {
            state: seed,
            events: seed,
            cameras: seed,
            tracks: seed,
            flights: seed,
        }
    }
}

/// The full snapshot stored behind the bridge mutex. Cloned cheaply each frame.
#[derive(Clone)]
pub struct Snapshot {
    pub view: ViewDto,
    pub show: ShowFlags,
    pub selected: SelectedDto,
    pub events: Vec<EventDto>,
    pub cameras: Vec<CameraDto>,
    // Already `Arc<Vec<…>>` in the model — clone is a refcount bump, not the data.
    pub tracks: Arc<Vec<MovingTrack>>,
    pub flights: Arc<Vec<FlightTrack>>,
    /// Per-slice change counters; not serialized — the server sends them as headers.
    pub gens: Generations,
}

impl Snapshot {
    /// Build the per-frame snapshot from the live model. Cheap by construction.
    pub fn from_model(model: &AppModel) -> Self {
        let centre = model.globe_view.globe_center_latlon();
        Snapshot {
            view: ViewDto {
                lat: centre.lat,
                lon: centre.lon,
                body: body_str(model.active_body),
                theme: model.map_theme.label(),
                local_mode: model.globe_view.local_mode,
            },
            show: ShowFlags {
                events: model.show_event_markers,
                ships: model.show_ships,
                flights: model.show_flights,
            },
            selected: SelectedDto {
                event: model.selected_event_id.clone(),
                camera: model.selected_camera_id.clone(),
                vessel: model.selected_track_mmsi,
                flight: model.selected_flight_icao24.clone(),
            },
            events: model.events.iter().map(event_dto).collect(),
            cameras: model.cameras.iter().map(camera_dto).collect(),
            tracks: model.tracks.clone(),
            flights: model.flights.clone(),
            gens: Generations::fresh(),
        }
    }

    /// Carry `prev`'s generation counters forward, bumping a slice's counter only
    /// when that slice actually changed. Runs once per frame in `publish`, so the
    /// comparisons stay cheap: pointer equality for the big Arc'd vecs, plain
    /// equality for the small DTO vecs and the scalar view state.
    pub fn carry_generations(&mut self, prev: &Snapshot) {
        let mut g = prev.gens;
        // `/state` serializes view + show + selected + the four slice counts.
        let counts_changed = self.events.len() != prev.events.len()
            || self.cameras.len() != prev.cameras.len()
            || self.tracks.len() != prev.tracks.len()
            || self.flights.len() != prev.flights.len();
        if counts_changed
            || self.view != prev.view
            || self.show != prev.show
            || self.selected != prev.selected
        {
            g.state += 1;
        }
        if self.events != prev.events {
            g.events += 1;
        }
        if self.cameras != prev.cameras {
            g.cameras += 1;
        }
        // Refreshed wholesale when a provider poll completes — pointer compare only.
        if !Arc::ptr_eq(&self.tracks, &prev.tracks) {
            g.tracks += 1;
        }
        if !Arc::ptr_eq(&self.flights, &prev.flights) {
            g.flights += 1;
        }
        self.gens = g;
    }

    /// `/state` — small, polled often: view + selection + layer flags + counts.
    pub fn state_json(&self) -> String {
        #[derive(Serialize)]
        struct StateDto<'a> {
            view: &'a ViewDto,
            show: &'a ShowFlags,
            selected: &'a SelectedDto,
            counts: Counts,
        }
        #[derive(Serialize)]
        struct Counts {
            events: usize,
            cameras: usize,
            tracks: usize,
            flights: usize,
        }
        let dto = StateDto {
            view: &self.view,
            show: &self.show,
            selected: &self.selected,
            counts: Counts {
                events: self.events.len(),
                cameras: self.cameras.len(),
                tracks: self.tracks.len(),
                flights: self.flights.len(),
            },
        };
        serde_json::to_string(&dto).unwrap_or_else(|_| "{}".into())
    }

    pub fn events_json(&self) -> String {
        serde_json::to_string(&self.events).unwrap_or_else(|_| "[]".into())
    }

    pub fn cameras_json(&self) -> String {
        serde_json::to_string(&self.cameras).unwrap_or_else(|_| "[]".into())
    }

    pub fn tracks_json(&self) -> String {
        let total = self.tracks.len();
        let items: Vec<TrackDto> = self.tracks.iter().take(MAX_MOVERS).map(track_dto).collect();
        let resp = ListResponse {
            total,
            truncated: total > items.len(),
            items,
        };
        serde_json::to_string(&resp).unwrap_or_else(|_| "{}".into())
    }

    pub fn flights_json(&self) -> String {
        let total = self.flights.len();
        let items: Vec<FlightDto> = self.flights.iter().take(MAX_MOVERS).map(flight_dto).collect();
        let resp = ListResponse {
            total,
            truncated: total > items.len(),
            items,
        };
        serde_json::to_string(&resp).unwrap_or_else(|_| "{}".into())
    }
}

fn body_str(body: ActiveBody) -> &'static str {
    match body {
        ActiveBody::Earth => "earth",
        ActiveBody::Moon => "moon",
        ActiveBody::Mars => "mars",
    }
}

fn event_dto(e: &EventRecord) -> EventDto {
    EventDto {
        id: e.id.clone(),
        title: e.title.clone(),
        summary: e.summary.clone(),
        severity: e.severity.label(),
        location_name: e.location_name.clone(),
        lat: e.location.lat,
        lon: e.location.lon,
        source: e.source.clone(),
        occurred_at: e.occurred_at.clone(),
    }
}

fn camera_dto(c: &CameraFeed) -> CameraDto {
    CameraDto {
        id: c.id.clone(),
        label: c.label.clone(),
        provider: c.provider.clone(),
        kind: c.kind.clone(),
        lat: c.location.lat,
        lon: c.location.lon,
        status: c.status.label(),
        last_seen: c.last_seen.clone(),
    }
}

fn track_dto(t: &MovingTrack) -> TrackDto {
    TrackDto {
        mmsi: t.mmsi,
        name: t.name.clone(),
        lat: t.location.lat,
        lon: t.location.lon,
        heading: t.heading_deg,
        speed: t.speed_knots,
        kind: t.ship_type_label(),
    }
}

fn flight_dto(f: &FlightTrack) -> FlightDto {
    FlightDto {
        icao24: f.icao24.clone(),
        callsign: f.callsign.clone(),
        lat: f.location.lat,
        lon: f.location.lon,
        heading: f.heading_deg,
        altitude: f.altitude_label(),
        category: flight_category_str(f),
        on_ground: f.on_ground,
    }
}

fn flight_category_str(f: &FlightTrack) -> &'static str {
    use crate::model::FlightCategory::*;
    match f.category() {
        Airline => "airline",
        Cargo => "cargo",
        Military => "military",
        GA => "ga",
        Unknown => "unknown",
    }
}

/// A command sent from a web viewer back to the host, drained on the UI thread.
#[derive(Debug, Clone)]
pub enum Command {
    /// Steer the host's globe to a coordinate ("look here").
    Focus { point: GeoPoint },
    /// Select an event by id on the host (and focus it).
    SelectEvent { id: String },
}
