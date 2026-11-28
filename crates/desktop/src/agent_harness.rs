//! Opt-in Loopback-Only (`127.0.0.1`) Local Agent HTTP/SSE Harness & Shared Map-Control Engine.
//!
//! Disabled by default. When explicitly enabled in Settings -> Agent & Voice,
//! binds strictly to `127.0.0.1:<port>` so local agents (e.g. Hermes or custom scripts)
//! can inspect live map state, events, webcams, and screenshots, and take full control
//! over the camera, Globe/Local views, layers, themes, viewshed, annotations, and voice:
//! - `GET  /v1/state`              — full camera, theme, layers, convergences & status snapshot
//! - `GET  /v1/events`             — live events with full summaries, severity & convergence scores
//! - `GET  /v1/cameras`            — webcams near the current view (or `?near=lat,lon,km`)
//! - `GET  /v1/screenshot`         — latest captured viewport JPEG (triggers capture if stale)
//! - `GET  /v1/capabilities`       — self-describing JSON manifest of all endpoints & layer names
//! - `GET  /v1/events/stream`      — live Server-Sent Events (focus changes & new events)
//! - `POST /v1/camera`             — smoothly fly camera, switch Globe/Local/Body, zoom, tilt, select event/camera
//! - `POST /v1/layers`             — switch theme, terrain analysis, viewshed, Atlas light, or toggle any map layer
//! - `POST /v1/narrate`            — speak via built-in ReverbTTS (with optional per-call voice/WAV override)
//! - `POST /v1/annotate`           — drop a glowing callout pin on the map
//! - `POST /v1/annotations/clear`  — remove all agent callout pins
//! - `POST /v1/suggest-stop`       — backwards-compatible alias to steer camera to a coordinate

use crate::atlas_style::LightMode;
use crate::model::{ActiveBody, AppModel, GeoPoint, TerrainAnalysisMode};
use crate::narrator::{Narrator, SpeechPriority, VoiceOverride};
use crate::theme::MapTheme;
use crossbeam_channel::{Receiver, Sender, unbounded};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct AgentAnnotation {
    pub location: GeoPoint,
    pub label: String,
    pub expires_at: Instant,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct CameraControl {
    pub lat: Option<f32>,
    pub lon: Option<f32>,
    /// `"globe"` or `"local"`.
    pub mode: Option<String>,
    /// `"earth"`, `"moon"`, or `"mars"`.
    pub body: Option<String>,
    /// Target globe zoom (`0.6..=50.0`) or local zoom (`0.5..=25.0`).
    pub zoom: Option<f32>,
    /// Local 3D terrain orbit yaw (radians).
    pub local_yaw: Option<f32>,
    /// Local 3D terrain tilt pitch (radians, `0.0` overhead to `1.45` low horizon).
    pub local_pitch: Option<f32>,
    /// Local 3D vertical relief spread (`1.0..=10.0`).
    pub layer_spread: Option<f32>,
    /// Smooth flight duration in seconds (`0.0` = instant, default `2.2`).
    pub duration_secs: Option<f32>,
    /// Optionally select an event by ID (also steers to it if `lat`/`lon` omitted).
    pub select_event_id: Option<String>,
    /// Optionally open the Factal brief window for the selected event.
    pub open_brief: Option<bool>,
    /// Optionally open a live webcam feed by ID.
    pub open_camera_id: Option<String>,
    /// Close the webcam feed window if true.
    pub close_camera: Option<bool>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct LayerControl {
    /// Theme name: `"Atlas"`, `"Topo"`, `"Abyss"`, `"Phosphor"`, `"Thermal"`, `"Ghost"`, `"Akira"`, `"Sodium"`, `"Lunar"`, `"Mars"`, `"MarsDark"`.
    pub theme: Option<String>,
    /// Analytical relief mode: `"off"`, `"slope"`, `"aspect"`, `"ridgelines"`.
    pub terrain_analysis: Option<String>,
    pub tanaka_contours: Option<bool>,
    pub contour_labels: Option<bool>,
    pub viewshed: Option<bool>,
    pub viewshed_follow_center: Option<bool>,
    pub viewshed_radius_km: Option<f32>,
    pub viewshed_observer_height_m: Option<f32>,
    pub events: Option<bool>,
    pub webcams: Option<bool>,
    pub camera_links: Option<bool>,
    pub coastlines: Option<bool>,
    pub bathymetry: Option<bool>,
    pub contours: Option<bool>,
    pub graticule: Option<bool>,
    pub reticle: Option<bool>,
    pub roads: Option<bool>,
    pub minor_roads: Option<bool>,
    pub railways: Option<bool>,
    pub airports: Option<bool>,
    pub water: Option<bool>,
    pub buildings: Option<bool>,
    pub trees: Option<bool>,
    pub admin_boundaries: Option<bool>,
    pub flights: Option<bool>,
    pub ships: Option<bool>,
    pub fires: Option<bool>,
    pub power: Option<bool>,
    pub military: Option<bool>,
    pub submarine_cables: Option<bool>,
    pub swot: Option<bool>,
    pub gravity: Option<bool>,
    pub lidar: Option<bool>,
    /// `"studio"` or `"sun"`.
    pub atlas_light_mode: Option<String>,
    pub atlas_azimuth_deg: Option<f32>,
    pub atlas_altitude_deg: Option<f32>,
    pub atlas_relief_strength: Option<f32>,
    pub follow_events: Option<bool>,
    pub meander: Option<bool>,
}

#[derive(Clone, Debug)]
pub enum AgentCommand {
    Camera(CameraControl),
    Layers(LayerControl),
    Narrate {
        text: String,
        source: String,
        priority: SpeechPriority,
        voice: Option<VoiceOverride>,
    },
    Annotate {
        location: GeoPoint,
        label: String,
        ttl_secs: u64,
    },
    ClearAnnotations,
    SuggestStop {
        location: GeoPoint,
        title: String,
    },
}

/// Smooth cubic ease-in-out autopilot glide driven by `AgentHarness::tick_flight`.
#[derive(Clone, Debug)]
pub struct AgentCameraFlight {
    started: Instant,
    duration: Duration,
    start_lat: f32,
    start_lon: f32,
    target_lat: f32,
    target_lon: f32,
    start_zoom: f32,
    target_zoom: f32,
    local_mode: bool,
    start_local_yaw: f32,
    target_local_yaw: f32,
    start_local_pitch: f32,
    target_local_pitch: f32,
    start_spread: f32,
    target_spread: f32,
}

impl AgentCameraFlight {
    pub fn tick(&self, model: &mut AppModel, now: Instant) -> bool {
        let dur = self.duration.as_secs_f32().max(0.001);
        let raw_t = (now.saturating_duration_since(self.started).as_secs_f32() / dur).clamp(0.0, 1.0);
        // Smoothstep / cubic ease-in-out
        let t = raw_t * raw_t * (3.0 - 2.0 * raw_t);

        let lat = self.start_lat + (self.target_lat - self.start_lat) * t;
        let dlon = ((self.target_lon - self.start_lon + 540.0).rem_euclid(360.0)) - 180.0;
        let lon = ((self.start_lon + dlon * t + 180.0).rem_euclid(360.0)) - 180.0;
        let point = GeoPoint {
            lat: lat.clamp(-85.0, 85.0),
            lon,
        };
        let zoom = (self.start_zoom.max(0.1).ln()
            + (self.target_zoom.max(0.1).ln() - self.start_zoom.max(0.1).ln()) * t)
            .exp();

        model.globe_view.local_mode = self.local_mode;
        model.globe_view.yaw = point.lon.to_radians() - std::f32::consts::FRAC_PI_2;
        model.globe_view.pitch = point.lat.to_radians().clamp(-1.48, 1.48);
        model.globe_view.local_center = point;
        if self.local_mode {
            model.globe_view.local_zoom = zoom.clamp(0.5, 25.0);
            model.globe_view.local_yaw =
                self.start_local_yaw + (self.target_local_yaw - self.start_local_yaw) * t;
            model.globe_view.local_pitch = (self.start_local_pitch
                + (self.target_local_pitch - self.start_local_pitch) * t)
                .clamp(0.0, 1.45);
            model.globe_view.local_layer_spread = (self.start_spread
                + (self.target_spread - self.start_spread) * t)
                .clamp(1.0, 10.0);
        } else {
            model.globe_view.zoom = zoom.clamp(0.6, 50.0);
        }
        raw_t < 1.0
    }

    pub fn into_orbit(&self, now: Instant) -> AgentFocusOrbit {
        AgentFocusOrbit {
            started: now,
            center_lat: self.target_lat,
            center_lon: self.target_lon,
            local_mode: self.local_mode,
            base_local_yaw: self.target_local_yaw,
            base_local_pitch: self.target_local_pitch,
            base_zoom: self.target_zoom,
        }
    }
}

/// Gentle continuous orbit around the agent's chosen point of focus once a flight arrives,
/// keeping both Local 3D Terrain and Globe views visually alive during narration & analysis.
#[derive(Clone, Debug)]
pub struct AgentFocusOrbit {
    started: Instant,
    center_lat: f32,
    center_lon: f32,
    local_mode: bool,
    base_local_yaw: f32,
    base_local_pitch: f32,
    base_zoom: f32,
}

impl AgentFocusOrbit {
    pub fn tick(&self, model: &mut AppModel, now: Instant) {
        let elapsed = now.saturating_duration_since(self.started).as_secs_f32();
        // Smooth ease-in over 1.8s so handing off from flight to orbit has zero jump.
        let ramp = (elapsed / 1.8).clamp(0.0, 1.0);
        let ease = ramp * ramp * (3.0 - 2.0 * ramp);

        if self.local_mode && model.globe_view.local_mode {
            // Smooth ~4.8°/sec panoramic orbit around the focus point with gentle pitch breath.
            let yaw_delta = elapsed * 0.085 * ease;
            let pitch_wave = (elapsed * 0.22).sin() * 0.045 * ease;
            model.globe_view.local_yaw = self.base_local_yaw + yaw_delta;
            model.globe_view.local_pitch = (self.base_local_pitch + pitch_wave).clamp(0.22, 1.38);
        } else if !self.local_mode && !model.globe_view.local_mode {
            // Gentle scale-aware orbital drift around the target on the Globe.
            let amp_deg = (5.5 / self.base_zoom.max(1.0)).clamp(0.4, 3.0) * ease;
            let lat = (self.center_lat + (elapsed * 0.16).sin() * amp_deg * 0.55).clamp(-84.0, 84.0);
            let lon = ((self.center_lon + (elapsed * 0.21).sin() * amp_deg + 180.0).rem_euclid(360.0))
                - 180.0;
            model.globe_view.yaw = lon.to_radians() - std::f32::consts::FRAC_PI_2;
            model.globe_view.pitch = lat.to_radians().clamp(-1.48, 1.48);
            model.globe_view.local_center = GeoPoint { lat, lon };
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct StateSnapshot {
    pub body: &'static str,
    pub local_mode: bool,
    pub center_lat: f32,
    pub center_lon: f32,
    pub zoom: f32,
    pub local_yaw: f32,
    pub local_pitch: f32,
    pub layer_spread: f32,
    pub theme: &'static str,
    pub terrain_analysis: &'static str,
    pub follow_events: bool,
    pub meander: bool,
    pub director_enabled: bool,
    pub director_status: String,
    pub narrator_status: String,
    pub utc: String,
    pub selected_event: Option<String>,
    pub selected_event_title: Option<String>,
    pub selected_event_summary: Option<String>,
    pub open_camera_id: Option<String>,
    pub event_count: usize,
    pub camera_count: usize,
    pub flight_count: usize,
    pub ship_count: usize,
    pub fire_count: usize,
    pub layers: LayersSnapshot,
    pub annotations: Vec<AnnotationDto>,
    pub convergences: Vec<ConvergenceDto>,
}

#[derive(Clone, Debug, Serialize)]
pub struct LayersSnapshot {
    pub events: bool,
    pub webcams: bool,
    pub coastlines: bool,
    pub bathymetry: bool,
    pub contours: bool,
    pub tanaka_contours: bool,
    pub contour_labels: bool,
    pub viewshed: bool,
    pub viewshed_follow_center: bool,
    pub graticule: bool,
    pub roads: bool,
    pub minor_roads: bool,
    pub railways: bool,
    pub airports: bool,
    pub water: bool,
    pub buildings: bool,
    pub trees: bool,
    pub admin_boundaries: bool,
    pub flights: bool,
    pub ships: bool,
    pub fires: bool,
    pub power: bool,
    pub military: bool,
    pub submarine_cables: bool,
    pub swot: bool,
    pub gravity: bool,
    pub lidar: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct AnnotationDto {
    pub lat: f32,
    pub lon: f32,
    pub label: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ConvergenceDto {
    pub id: String,
    pub title: String,
    pub lat: f32,
    pub lon: f32,
    pub score: f32,
    pub badge: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct EventDto {
    pub id: String,
    pub title: String,
    pub summary: String,
    pub severity: &'static str,
    pub location_name: String,
    pub lat: f32,
    pub lon: f32,
    pub occurred_at: String,
    pub convergence_score: Option<f32>,
}

#[derive(Clone, Debug, Serialize)]
pub struct CameraDto {
    pub id: String,
    pub label: String,
    pub provider: String,
    pub kind: String,
    pub lat: f32,
    pub lon: f32,
    pub distance_km: f32,
}

#[derive(Deserialize)]
struct NarrateReq {
    text: String,
    #[serde(default)]
    priority: Option<String>,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    voice_preset: Option<String>,
    #[serde(default)]
    ref_audio: Option<String>,
    #[serde(default)]
    ref_text: Option<String>,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    tts_voice: Option<String>,
    #[serde(default)]
    tts_instruction: Option<String>,
    #[serde(default)]
    seed: Option<i64>,
}

#[derive(Deserialize)]
struct AnnotateReq {
    lat: f32,
    lon: f32,
    label: String,
    #[serde(default)]
    ttl_secs: Option<u64>,
}

#[derive(Deserialize)]
struct SuggestStopReq {
    lat: f32,
    lon: f32,
    #[serde(default)]
    title: Option<String>,
}

struct SharedSnapshots {
    state_json: Mutex<String>,
    events_json: Mutex<String>,
    cameras_json: Mutex<String>,
    screenshot_jpeg: Mutex<Option<Vec<u8>>>,
    screenshot_wanted: AtomicBool,
}

struct RunningServer {
    port: u16,
    stop: Arc<AtomicBool>,
    snapshots: Arc<SharedSnapshots>,
    sse_clients: Arc<Mutex<Vec<Sender<String>>>>,
    cmd_rx: Receiver<AgentCommand>,
}

pub struct AgentHarness {
    server: Option<RunningServer>,
    last_event_id: Option<String>,
    last_event_count: usize,
    active_flight: Option<AgentCameraFlight>,
    active_orbit: Option<AgentFocusOrbit>,
    pub status: String,
}

impl Default for AgentHarness {
    fn default() -> Self {
        Self {
            server: None,
            last_event_id: None,
            last_event_count: 0,
            active_flight: None,
            active_orbit: None,
            status: "off".into(),
        }
    }
}

impl Drop for AgentHarness {
    fn drop(&mut self) {
        self.stop();
    }
}

impl AgentHarness {
    pub fn sync_lifecycle(&mut self, enabled: bool, port: u16) {
        if !enabled {
            if self.server.is_some() {
                self.stop();
            }
            self.status = "off".into();
            return;
        }
        if self.server.as_ref().is_some_and(|s| s.port == port) {
            return;
        }
        self.stop();
        match start_loopback_server(port) {
            Ok(srv) => {
                self.status = format!("listening on http://127.0.0.1:{}", srv.port);
                self.server = Some(srv);
            }
            Err(err) => {
                self.status = format!("bind failed: {err}");
            }
        }
    }

    pub fn stop(&mut self) {
        if let Some(srv) = self.server.take() {
            srv.stop.store(true, Ordering::Release);
            let _ = TcpStream::connect_timeout(
                &std::net::SocketAddr::from(([127, 0, 0, 1], srv.port)),
                Duration::from_millis(50),
            );
        }
    }

    /// Apply a command from either the loopback HTTP API or the built-in AI Agent Director.
    pub fn apply_command(
        &mut self,
        cmd: AgentCommand,
        model: &mut AppModel,
        narrator: &mut Narrator,
    ) {
        apply_agent_command(
            cmd,
            model,
            narrator,
            &mut self.active_flight,
            &mut self.active_orbit,
        );
    }

    /// True if an HTTP client requested `/v1/screenshot` and awaits a fresh frame.
    pub fn wants_screenshot(&self) -> bool {
        self.server
            .as_ref()
            .is_some_and(|s| s.snapshots.screenshot_wanted.swap(false, Ordering::AcqRel))
    }

    /// Store the latest captured JPEG screenshot for `/v1/screenshot`.
    pub fn set_latest_screenshot(&self, jpeg: Vec<u8>) {
        if let Some(srv) = &self.server
            && let Ok(mut guard) = srv.snapshots.screenshot_jpeg.lock()
        {
            *guard = Some(jpeg);
        }
    }

    /// Cancel any active agent camera glide or focus orbit (e.g. when the operator grabs the map).
    pub fn cancel_flight(&mut self) {
        self.active_flight = None;
        self.active_orbit = None;
    }

    pub fn publish_and_drain(&mut self, model: &mut AppModel, narrator: &mut Narrator) {
        // Expire old annotations regardless of whether HTTP server is running.
        let now = Instant::now();
        model.agent_annotations.retain(|a| a.expires_at > now);

        // Advance any active smooth agent camera flight, handing off seamlessly into a gentle orbit.
        if let Some(flight) = &self.active_flight {
            if flight.tick(model, now) {
                crate::app::request_repaint();
            } else {
                self.active_orbit = Some(flight.into_orbit(now));
                self.active_flight = None;
                crate::app::request_repaint();
            }
        } else if let Some(orbit) = &self.active_orbit {
            orbit.tick(model, now);
            crate::app::request_repaint();
        }

        let Some(srv) = &self.server else {
            return;
        };

        // Drain incoming commands from external local agents (HTTP).
        let pending: Vec<AgentCommand> = srv.cmd_rx.try_iter().collect();
        for cmd in pending {
            apply_agent_command(
                cmd,
                model,
                narrator,
                &mut self.active_flight,
                &mut self.active_orbit,
            );
        }

        // Refresh JSON snapshots for GET /v1/state, /v1/events, /v1/cameras.
        let state = build_state_snapshot(model);
        if let Ok(json) = serde_json::to_string(&state)
            && let Ok(mut guard) = srv.snapshots.state_json.lock()
        {
            *guard = json;
        }
        let events = build_event_dtos(model, 40);
        if let Ok(json) = serde_json::to_string(&events)
            && let Ok(mut guard) = srv.snapshots.events_json.lock()
        {
            *guard = json;
        }
        let center = if model.globe_view.local_mode {
            model.globe_view.local_center
        } else {
            model.globe_view.globe_center_latlon()
        };
        let cameras = build_camera_dtos(model, center, 25);
        if let Ok(json) = serde_json::to_string(&cameras)
            && let Ok(mut guard) = srv.snapshots.cameras_json.lock()
        {
            *guard = json;
        }

        // Emit SSE notifications on focus change or newly arrived events.
        if self.last_event_id != model.selected_event_id {
            self.last_event_id = model.selected_event_id.clone();
            if let Some(ev) = model.selected_event() {
                let badge = model.convergence.badge_for_event(&ev.id);
                let payload = serde_json::json!({
                    "type": "focus_changed",
                    "id": ev.id,
                    "title": ev.title,
                    "summary": crate::narrator::sanitize_for_speech(&ev.summary),
                    "location_name": ev.location_name,
                    "lat": ev.location.lat,
                    "lon": ev.location.lon,
                    "badge": badge,
                })
                .to_string();
                if let Ok(mut clients) = srv.sse_clients.lock() {
                    clients.retain(|tx| tx.send(payload.clone()).is_ok());
                }
            }
        }
        if model.events.len() > self.last_event_count {
            if let Some(newest) = model.events.first() {
                let payload = serde_json::json!({
                    "type": "new_event",
                    "id": newest.id,
                    "title": newest.title,
                    "summary": crate::narrator::sanitize_for_speech(&newest.summary),
                    "location_name": newest.location_name,
                    "lat": newest.location.lat,
                    "lon": newest.location.lon,
                })
                .to_string();
                if let Ok(mut clients) = srv.sse_clients.lock() {
                    clients.retain(|tx| tx.send(payload.clone()).is_ok());
                }
            }
        }
        self.last_event_count = model.events.len();
    }
}

pub fn build_state_snapshot(model: &AppModel) -> StateSnapshot {
    let center = if model.globe_view.local_mode {
        model.globe_view.local_center
    } else {
        model.globe_view.globe_center_latlon()
    };
    StateSnapshot {
        body: match model.active_body {
            ActiveBody::Earth => "earth",
            ActiveBody::Moon => "moon",
            ActiveBody::Mars => "mars",
        },
        local_mode: model.globe_view.local_mode,
        center_lat: center.lat,
        center_lon: center.lon,
        zoom: if model.globe_view.local_mode {
            model.globe_view.local_zoom
        } else {
            model.globe_view.zoom
        },
        local_yaw: model.globe_view.local_yaw,
        local_pitch: model.globe_view.local_pitch,
        layer_spread: model.globe_view.local_layer_spread,
        theme: model.map_theme.label(),
        terrain_analysis: model.terrain_analysis_mode.label(),
        follow_events: model.event_follow.enabled(),
        meander: model.globe_view.meander_mode,
        director_enabled: model.agent_voice.director_enabled,
        director_status: model.agent_director_status.clone(),
        narrator_status: model.narrator_status.clone(),
        utc: model.master_clock.formatted_utc(),
        selected_event: model.selected_event_id.clone(),
        selected_event_title: model.selected_event().map(|e| e.title.clone()),
        selected_event_summary: model
            .selected_event()
            .map(|e| crate::narrator::sanitize_for_speech(&e.summary)),
        open_camera_id: model
            .camera_feed_window_open
            .then(|| model.selected_camera_id.clone())
            .flatten(),
        event_count: model.events.len(),
        camera_count: model.cameras.len(),
        flight_count: model.flights.len(),
        ship_count: model.tracks.len(),
        fire_count: model.active_fires.len(),
        layers: LayersSnapshot {
            events: model.show_event_markers,
            webcams: model.show_camera_markers,
            coastlines: model.show_coastlines,
            bathymetry: model.show_bathymetry,
            contours: model.show_contours,
            tanaka_contours: model.tanaka_contours,
            contour_labels: model.show_contour_labels,
            viewshed: model.viewshed.enabled,
            viewshed_follow_center: model.viewshed.follow_center,
            graticule: model.show_graticule,
            roads: model.show_major_roads,
            minor_roads: model.show_minor_roads,
            railways: model.show_rail,
            airports: model.show_aeroway,
            water: model.show_water,
            buildings: model.show_buildings,
            trees: model.show_trees,
            admin_boundaries: model.show_admin,
            flights: model.show_flights,
            ships: model.show_ships,
            fires: model.show_active_fires,
            power: model.show_power,
            military: model.show_military,
            submarine_cables: model.show_submarine_cables,
            swot: model.show_swot_gravity,
            gravity: model.gravity.enabled,
            lidar: model.lidar.enabled,
        },
        annotations: model
            .agent_annotations
            .iter()
            .map(|a| AnnotationDto {
                lat: a.location.lat,
                lon: a.location.lon,
                label: a.label.clone(),
            })
            .collect(),
        convergences: model
            .convergence
            .clusters()
            .iter()
            .take(8)
            .map(|c| ConvergenceDto {
                id: c.anchor_event_id.clone(),
                title: c.anchor_event.title.clone(),
                lat: c.center.lat,
                lon: c.center.lon,
                score: c.priority_score,
                badge: c.badge_text(),
            })
            .collect(),
    }
}

pub fn build_event_dtos(model: &AppModel, limit: usize) -> Vec<EventDto> {
    model
        .events
        .iter()
        .take(limit)
        .map(|ev| {
            let score = model
                .convergence
                .clusters()
                .iter()
                .find(|c| c.anchor_event_id == ev.id)
                .map(|c| c.priority_score);
            EventDto {
                id: ev.id.clone(),
                title: ev.title.clone(),
                summary: crate::narrator::sanitize_for_speech(&ev.summary),
                severity: ev.severity.label(),
                location_name: ev.location_name.clone(),
                lat: ev.location.lat,
                lon: ev.location.lon,
                occurred_at: ev.occurred_at.clone(),
                convergence_score: score,
            }
        })
        .collect()
}

pub fn build_camera_dtos(model: &AppModel, center: GeoPoint, limit: usize) -> Vec<CameraDto> {
    let mut ranked: Vec<(f32, &crate::model::CameraFeed)> = model
        .cameras
        .iter()
        .map(|cam| (haversine_km(center, cam.location), cam))
        .collect();
    ranked.sort_unstable_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    ranked
        .into_iter()
        .take(limit)
        .map(|(dist, cam)| CameraDto {
            id: cam.id.clone(),
            label: cam.label.clone(),
            provider: cam.provider.clone(),
            kind: cam.kind.clone(),
            lat: cam.location.lat,
            lon: cam.location.lon,
            distance_km: (dist * 10.0).round() / 10.0,
        })
        .collect()
}

fn haversine_km(a: GeoPoint, b: GeoPoint) -> f32 {
    let r = 6371.0_f32;
    let dlat = (b.lat - a.lat).to_radians();
    let dlon = (b.lon - a.lon).to_radians();
    let la1 = a.lat.to_radians();
    let la2 = b.lat.to_radians();
    let h = (dlat * 0.5).sin().powi(2) + la1.cos() * la2.cos() * (dlon * 0.5).sin().powi(2);
    2.0 * r * h.sqrt().min(1.0).asin()
}

pub fn apply_agent_command(
    cmd: AgentCommand,
    model: &mut AppModel,
    narrator: &mut Narrator,
    active_flight: &mut Option<AgentCameraFlight>,
    active_orbit: &mut Option<AgentFocusOrbit>,
) {
    match cmd {
        AgentCommand::Camera(cam) => {
            apply_camera_control(cam, model, active_flight, active_orbit);
        }
        AgentCommand::Layers(layers) => {
            apply_layer_control(layers, model);
        }
        AgentCommand::Narrate {
            text,
            source,
            priority,
            voice,
        } => {
            narrator.enqueue_with_voice(text, source, priority, &model.agent_voice, voice);
        }
        AgentCommand::Annotate {
            location,
            label,
            ttl_secs,
        } => {
            model.agent_annotations.push(AgentAnnotation {
                location,
                label: label.clone(),
                expires_at: Instant::now() + Duration::from_secs(ttl_secs.clamp(5, 3600)),
            });
            if model.agent_annotations.len() > 32 {
                model.agent_annotations.remove(0);
            }
            model.push_log(format!("Agent annotated map: {label}"));
        }
        AgentCommand::ClearAnnotations => {
            model.agent_annotations.clear();
            model.push_log("Agent cleared map annotations".into());
        }
        AgentCommand::SuggestStop { location, title } => {
            apply_camera_control(
                CameraControl {
                    lat: Some(location.lat),
                    lon: Some(location.lon),
                    ..Default::default()
                },
                model,
                active_flight,
                active_orbit,
            );
            model.push_log(format!(
                "Agent steered map to {title} ({:.2}°, {:.2}°)",
                location.lat, location.lon
            ));
        }
    }
    crate::app::request_repaint();
}

fn apply_camera_control(
    cam: CameraControl,
    model: &mut AppModel,
    active_flight: &mut Option<AgentCameraFlight>,
    active_orbit: &mut Option<AgentFocusOrbit>,
) {
    // Switching planetary body
    if let Some(body) = cam.body.as_deref() {
        let next = match body.trim().to_ascii_lowercase().as_str() {
            "earth" => Some(ActiveBody::Earth),
            "moon" => Some(ActiveBody::Moon),
            "mars" => Some(ActiveBody::Mars),
            _ => None,
        };
        if let Some(next_body) = next
            && next_body != model.active_body
        {
            model.stop_event_follow();
            model.active_body = next_body;
            if next_body != ActiveBody::Earth {
                model.globe_view.local_mode = false;
            }
            model.map_theme = match next_body {
                ActiveBody::Earth => MapTheme::Atlas,
                ActiveBody::Moon => MapTheme::Lunar,
                ActiveBody::Mars => MapTheme::MarsDark,
            };
        }
    }

    // Optional event selection
    let mut event_point: Option<GeoPoint> = None;
    if let Some(event_id) = cam.select_event_id.as_deref()
        && let Some(ev) = model.events.iter().find(|e| e.id == event_id)
    {
        event_point = Some(ev.location);
        model.selected_event_id = Some(ev.id.clone());
        if cam.open_brief == Some(true) && ev.factal_brief.is_some() {
            model.factal_brief_open = true;
        }
    }
    if cam.open_brief == Some(false) {
        model.factal_brief_open = false;
    }

    // Optional webcam modal
    if cam.close_camera == Some(true) {
        model.camera_feed_window_open = false;
    } else if let Some(cam_id) = cam.open_camera_id.as_deref()
        && model.cameras.iter().any(|c| c.id == cam_id)
    {
        model.selected_camera_id = Some(cam_id.to_owned());
        model.camera_feed_window_open = true;
    }

    let wants_pose_change = cam.lat.is_some()
        || cam.lon.is_some()
        || event_point.is_some()
        || cam.mode.is_some()
        || cam.zoom.is_some()
        || cam.local_yaw.is_some()
        || cam.local_pitch.is_some()
        || cam.layer_spread.is_some();
    if !wants_pose_change {
        return;
    }

    model.stop_event_follow();
    model.globe_view.stop_motion();

    let cur_center = if model.globe_view.local_mode {
        model.globe_view.local_center
    } else {
        model.globe_view.globe_center_latlon()
    };

    let target_local_mode = match cam.mode.as_deref().map(|s| s.trim().to_ascii_lowercase()) {
        Some(ref s) if s == "local" => true,
        Some(ref s) if s == "globe" => false,
        _ => model.globe_view.local_mode,
    };

    let target_lat = cam
        .lat
        .filter(|v| v.is_finite())
        .or_else(|| event_point.map(|p| p.lat))
        .unwrap_or(cur_center.lat)
        .clamp(-85.0, 85.0);
    let target_lon = cam
        .lon
        .filter(|v| v.is_finite())
        .or_else(|| event_point.map(|p| p.lon))
        .unwrap_or(cur_center.lon);
    let target_lon = ((target_lon + 180.0).rem_euclid(360.0)) - 180.0;

    let start_zoom = if target_local_mode {
        model.globe_view.local_zoom
    } else {
        model.globe_view.zoom
    };
    let target_zoom = cam
        .zoom
        .filter(|v| v.is_finite() && *v > 0.0)
        .map(|z| {
            if target_local_mode {
                z.clamp(0.5, 25.0)
            } else {
                z.clamp(0.6, 50.0)
            }
        })
        .unwrap_or_else(|| {
            if target_local_mode != model.globe_view.local_mode {
                if target_local_mode { 10.0 } else { 2.6 }
            } else {
                start_zoom
            }
        });

    let target_local_yaw = cam
        .local_yaw
        .filter(|v| v.is_finite())
        .unwrap_or(model.globe_view.local_yaw);
    let target_local_pitch = cam
        .local_pitch
        .filter(|v| v.is_finite())
        .unwrap_or(model.globe_view.local_pitch)
        .clamp(0.0, 1.45);
    let target_spread = cam
        .layer_spread
        .filter(|v| v.is_finite())
        .unwrap_or(model.globe_view.local_layer_spread)
        .clamp(1.0, 10.0);

    let duration_secs = cam
        .duration_secs
        .filter(|v| v.is_finite())
        .unwrap_or(2.2)
        .clamp(0.0, 10.0);

    // Switching mode (Globe <-> Local) jumps cleanly on arrival or immediately if duration == 0.
    if duration_secs <= 0.05 || target_local_mode != model.globe_view.local_mode {
        let point = GeoPoint {
            lat: target_lat,
            lon: target_lon,
        };
        model.globe_view.local_mode = target_local_mode;
        model.globe_view.focus_on(point);
        if target_local_mode {
            model.globe_view.local_zoom = target_zoom;
            model.globe_view.local_yaw = target_local_yaw;
            model.globe_view.local_pitch = target_local_pitch;
            model.globe_view.local_layer_spread = target_spread;
        } else {
            model.globe_view.zoom = target_zoom;
        }
        *active_flight = None;
        *active_orbit = Some(AgentFocusOrbit {
            started: Instant::now(),
            center_lat: target_lat,
            center_lon: target_lon,
            local_mode: target_local_mode,
            base_local_yaw: target_local_yaw,
            base_local_pitch: target_local_pitch,
            base_zoom: target_zoom,
        });
    } else {
        *active_orbit = None;
        *active_flight = Some(AgentCameraFlight {
            started: Instant::now(),
            duration: Duration::from_secs_f32(duration_secs),
            start_lat: cur_center.lat,
            start_lon: cur_center.lon,
            target_lat,
            target_lon,
            start_zoom,
            target_zoom,
            local_mode: target_local_mode,
            start_local_yaw: model.globe_view.local_yaw,
            target_local_yaw,
            start_local_pitch: model.globe_view.local_pitch,
            target_local_pitch,
            start_spread: model.globe_view.local_layer_spread,
            target_spread,
        });
    }
}

fn parse_theme(name: &str) -> Option<MapTheme> {
    let key = name.trim().to_ascii_lowercase().replace([' ', '_', '-'], "");
    match key.as_str() {
        "topo" => Some(MapTheme::Topo),
        "atlas" => Some(MapTheme::Atlas),
        "abyss" => Some(MapTheme::Abyss),
        "phosphor" => Some(MapTheme::Phosphor),
        "thermal" => Some(MapTheme::Thermal),
        "ghost" => Some(MapTheme::Ghost),
        "akira" => Some(MapTheme::Akira),
        "sodium" => Some(MapTheme::Sodium),
        "lunar" => Some(MapTheme::Lunar),
        "mars" => Some(MapTheme::Mars),
        "marsdark" => Some(MapTheme::MarsDark),
        _ => None,
    }
}

fn apply_layer_control(l: LayerControl, model: &mut AppModel) {
    if let Some(theme_name) = l.theme.as_deref()
        && let Some(theme) = parse_theme(theme_name)
    {
        model.map_theme = theme;
    }
    if let Some(mode_name) = l.terrain_analysis.as_deref() {
        model.terrain_analysis_mode = match mode_name.trim().to_ascii_lowercase().as_str() {
            "slope" => TerrainAnalysisMode::Slope,
            "aspect" => TerrainAnalysisMode::Aspect,
            "ridgeline" | "ridgelines" | "tpi" => TerrainAnalysisMode::Ridgeline,
            _ => TerrainAnalysisMode::None,
        };
    }
    if let Some(v) = l.tanaka_contours {
        model.tanaka_contours = v;
    }
    if let Some(v) = l.contour_labels {
        model.show_contour_labels = v;
    }
    if let Some(v) = l.viewshed {
        if v {
            model.viewshed.set_follow_center();
        } else {
            model.viewshed.clear();
        }
    }
    if let Some(v) = l.viewshed_follow_center {
        model.viewshed.follow_center = v;
        model.viewshed.invalidate();
    }
    if let Some(r) = l.viewshed_radius_km.filter(|v| v.is_finite()) {
        model.viewshed.max_range_km = r.clamp(2.0, 120.0);
        model.viewshed.invalidate();
    }
    if let Some(h) = l.viewshed_observer_height_m.filter(|v| v.is_finite()) {
        model.viewshed.observer_agl_m = h.clamp(2.0, 2000.0);
        model.viewshed.invalidate();
    }
    if let Some(v) = l.events { model.show_event_markers = v; }
    if let Some(v) = l.webcams { model.show_camera_markers = v; }
    if let Some(v) = l.camera_links { model.show_camera_links = v; }
    if let Some(v) = l.coastlines { model.show_coastlines = v; }
    if let Some(v) = l.bathymetry { model.show_bathymetry = v; }
    if let Some(v) = l.contours { model.show_contours = v; }
    if let Some(v) = l.graticule { model.show_graticule = v; }
    if let Some(v) = l.reticle { model.show_reticle = v; }
    if let Some(v) = l.roads { model.show_major_roads = v; }
    if let Some(v) = l.minor_roads { model.show_minor_roads = v; }
    if let Some(v) = l.railways { model.show_rail = v; }
    if let Some(v) = l.airports { model.show_aeroway = v; }
    if let Some(v) = l.water { model.show_water = v; }
    if let Some(v) = l.buildings { model.show_buildings = v; }
    if let Some(v) = l.trees { model.show_trees = v; }
    if let Some(v) = l.admin_boundaries { model.show_admin = v; }
    if let Some(v) = l.flights { model.show_flights = v; }
    if let Some(v) = l.ships { model.show_ships = v; }
    if let Some(v) = l.fires { model.show_active_fires = v; }
    if let Some(v) = l.power { model.show_power = v; }
    if let Some(v) = l.military { model.show_military = v; }
    if let Some(v) = l.submarine_cables { model.show_submarine_cables = v; }
    if let Some(v) = l.swot { model.show_swot_gravity = v; }
    if let Some(v) = l.gravity { model.gravity.enabled = v; }
    if let Some(v) = l.lidar { model.lidar.enabled = v; }
    if let Some(mode) = l.atlas_light_mode.as_deref() {
        match mode.trim().to_ascii_lowercase().as_str() {
            "sun" | "sunlight" => model.atlas_style.light_mode = LightMode::Sun,
            "studio" | "atlas" => model.atlas_style.light_mode = LightMode::Studio,
            _ => {}
        }
    }
    if let Some(az) = l.atlas_azimuth_deg.filter(|v| v.is_finite()) {
        model.atlas_style.azimuth_deg = az.rem_euclid(360.0);
    }
    if let Some(alt) = l.atlas_altitude_deg.filter(|v| v.is_finite()) {
        model.atlas_style.altitude_deg = alt.clamp(5.0, 85.0);
    }
    if let Some(rel) = l.atlas_relief_strength.filter(|v| v.is_finite()) {
        model.atlas_style.relief_strength = rel.clamp(0.0, 2.0);
    }
    model.atlas_style = model.atlas_style.normalized();
    if let Some(v) = l.follow_events {
        if v && !model.event_follow.enabled() {
            model.toggle_event_follow();
        } else if !v && model.event_follow.enabled() {
            model.stop_event_follow();
        }
    }
    if let Some(v) = l.meander {
        if v {
            model.stop_event_follow();
        }
        model.globe_view.meander_mode = v;
    }
}

fn capabilities_json() -> &'static str {
    r#"{
  "version": "1.1",
  "endpoints": {
    "GET /v1/state": "Full camera pose, theme, active layers, annotations, convergences, and status.",
    "GET /v1/events": "Top live events with summaries, severity, coordinates, and convergence scores.",
    "GET /v1/cameras": "Nearest live webcams to current focus with IDs and distances.",
    "GET /v1/screenshot": "Latest map viewport JPEG image (image/jpeg).",
    "GET /v1/events/stream": "Server-Sent Events stream for focus_changed and new_event.",
    "POST /v1/camera": "Body JSON: { lat, lon, mode ('globe'|'local'), body ('earth'|'moon'|'mars'), zoom, local_yaw, local_pitch, layer_spread, duration_secs, select_event_id, open_brief, open_camera_id, close_camera }",
    "POST /v1/layers": "Body JSON: { theme, terrain_analysis ('off'|'slope'|'aspect'|'ridgelines'), tanaka_contours, contour_labels, viewshed, viewshed_follow_center, viewshed_radius_km, viewshed_observer_height_m, events, webcams, camera_links, coastlines, bathymetry, contours, graticule, reticle, roads, minor_roads, railways, airports, water, buildings, trees, admin_boundaries, flights, ships, fires, power, military, submarine_cables, swot, gravity, lidar, atlas_light_mode ('studio'|'sun'), atlas_azimuth_deg, atlas_altitude_deg, atlas_relief_strength, follow_events, meander }",
    "POST /v1/narrate": "Body JSON: { text, source?, priority? ('normal'|'high'), voice_preset?, ref_audio?, ref_text?, language?, seed? }",
    "POST /v1/annotate": "Body JSON: { lat, lon, label, ttl_secs? }",
    "POST /v1/annotations/clear": "Clears all agent map annotations."
  },
  "themes": ["Atlas", "Topo", "Abyss", "Phosphor", "Thermal", "Ghost", "Akira", "Sodium", "Lunar", "Mars", "MarsDark"]
}"#
}

fn start_loopback_server(port: u16) -> Result<RunningServer, String> {
    let listener = TcpListener::bind(("127.0.0.1", port)).map_err(|e| e.to_string())?;
    let actual_port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let stop = Arc::new(AtomicBool::new(false));
    let snapshots = Arc::new(SharedSnapshots {
        state_json: Mutex::new("{}".to_owned()),
        events_json: Mutex::new("[]".to_owned()),
        cameras_json: Mutex::new("[]".to_owned()),
        screenshot_jpeg: Mutex::new(None),
        screenshot_wanted: AtomicBool::new(false),
    });
    let sse_clients: Arc<Mutex<Vec<Sender<String>>>> = Arc::new(Mutex::new(Vec::new()));
    let (cmd_tx, cmd_rx) = unbounded::<AgentCommand>();

    let w_stop = Arc::clone(&stop);
    let w_snaps = Arc::clone(&snapshots);
    let w_sse = Arc::clone(&sse_clients);

    std::thread::Builder::new()
        .name("1kee-agent-harness".into())
        .spawn(move || {
            for incoming in listener.incoming() {
                if w_stop.load(Ordering::Acquire) {
                    break;
                }
                let Ok(stream) = incoming else {
                    continue;
                };
                let snaps = Arc::clone(&w_snaps);
                let sse = Arc::clone(&w_sse);
                let tx = cmd_tx.clone();
                let stop_flag = Arc::clone(&w_stop);
                std::thread::spawn(move || {
                    handle_connection(stream, &snaps, &sse, &tx, &stop_flag);
                });
            }
        })
        .map_err(|e| e.to_string())?;

    Ok(RunningServer {
        port: actual_port,
        stop,
        snapshots,
        sse_clients,
        cmd_rx,
    })
}

fn handle_connection(
    mut stream: TcpStream,
    snapshots: &SharedSnapshots,
    sse_clients: &Mutex<Vec<Sender<String>>>,
    cmd_tx: &Sender<AgentCommand>,
    stop: &AtomicBool,
) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let Ok(clone) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(clone);
    let mut req_line = String::new();
    if reader.read_line(&mut req_line).is_err() || req_line.trim().is_empty() {
        return;
    }
    let parts: Vec<&str> = req_line.split_whitespace().collect();
    if parts.len() < 2 {
        return;
    }
    let method = parts[0];
    let path = parts[1].split('?').next().unwrap_or(parts[1]);

    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() || line.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':')
            && k.trim().eq_ignore_ascii_case("content-length")
        {
            content_length = v.trim().parse::<usize>().unwrap_or(0).min(128 * 1024);
        }
    }

    let mut body = vec![0u8; content_length];
    if content_length > 0 && reader.read_exact(&mut body).is_err() {
        write_json(&mut stream, 400, r#"{"error":"incomplete body"}"#);
        return;
    }

    match (method, path) {
        ("GET", "/v1/state") => {
            let json = snapshots
                .state_json
                .lock()
                .map(|g| g.clone())
                .unwrap_or_else(|_| "{}".into());
            write_json(&mut stream, 200, &json);
        }
        ("GET", "/v1/events") => {
            let json = snapshots
                .events_json
                .lock()
                .map(|g| g.clone())
                .unwrap_or_else(|_| "[]".into());
            write_json(&mut stream, 200, &json);
        }
        ("GET", "/v1/cameras") => {
            let json = snapshots
                .cameras_json
                .lock()
                .map(|g| g.clone())
                .unwrap_or_else(|_| "[]".into());
            write_json(&mut stream, 200, &json);
        }
        ("GET", "/v1/capabilities") => {
            write_json(&mut stream, 200, capabilities_json());
        }
        ("GET", "/v1/screenshot") => {
            snapshots.screenshot_wanted.store(true, Ordering::Release);
            crate::app::request_repaint();
            // Briefly wait if no screenshot has been captured yet.
            for _ in 0..10 {
                if snapshots
                    .screenshot_jpeg
                    .lock()
                    .ok()
                    .is_some_and(|g| g.is_some())
                {
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            let bytes = snapshots
                .screenshot_jpeg
                .lock()
                .ok()
                .and_then(|g| g.clone());
            if let Some(jpeg) = bytes {
                write_bytes(&mut stream, 200, "image/jpeg", &jpeg);
            } else {
                write_json(&mut stream, 503, r#"{"error":"screenshot not yet ready"}"#);
            }
        }
        ("GET", "/v1/events/stream") => {
            let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
            let header = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: keep-alive\r\n\r\n: connected\n\n";
            if stream.write_all(header.as_bytes()).is_err() {
                return;
            }
            let (tx, rx) = unbounded::<String>();
            if let Ok(mut clients) = sse_clients.lock() {
                clients.push(tx);
            }
            while !stop.load(Ordering::Acquire) {
                match rx.recv_timeout(Duration::from_secs(10)) {
                    Ok(msg) => {
                        let frame = format!("data: {msg}\n\n");
                        if stream.write_all(frame.as_bytes()).is_err() {
                            break;
                        }
                    }
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                        if stream.write_all(b": keepalive\n\n").is_err() {
                            break;
                        }
                    }
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                }
            }
        }
        ("POST", "/v1/camera") => match serde_json::from_slice::<CameraControl>(&body) {
            Ok(cam) => {
                let _ = cmd_tx.send(AgentCommand::Camera(cam));
                crate::app::request_repaint();
                write_json(&mut stream, 200, r#"{"ok":true}"#);
            }
            Err(err) => {
                let msg = serde_json::json!({ "error": format!("invalid camera JSON: {err}") }).to_string();
                write_json(&mut stream, 400, &msg);
            }
        },
        ("POST", "/v1/layers") => match serde_json::from_slice::<LayerControl>(&body) {
            Ok(layers) => {
                let _ = cmd_tx.send(AgentCommand::Layers(layers));
                crate::app::request_repaint();
                write_json(&mut stream, 200, r#"{"ok":true}"#);
            }
            Err(err) => {
                let msg = serde_json::json!({ "error": format!("invalid layers JSON: {err}") }).to_string();
                write_json(&mut stream, 400, &msg);
            }
        },
        ("POST", "/v1/narrate") => match serde_json::from_slice::<NarrateReq>(&body) {
            Ok(req) if !req.text.trim().is_empty() => {
                let priority = if req
                    .priority
                    .as_deref()
                    .is_some_and(|p| p.eq_ignore_ascii_case("high"))
                {
                    SpeechPriority::High
                } else {
                    SpeechPriority::Normal
                };
                let has_override = req.voice_preset.is_some()
                    || req.ref_audio.is_some()
                    || req.ref_text.is_some()
                    || req.language.is_some()
                    || req.tts_voice.is_some()
                    || req.tts_instruction.is_some()
                    || req.seed.is_some();
                let voice = has_override.then_some(VoiceOverride {
                    reverb_voice_preset: req.voice_preset,
                    ref_audio: req.ref_audio,
                    ref_text: req.ref_text,
                    reverb_language: req.language,
                    tts_voice: req.tts_voice,
                    tts_instruction: req.tts_instruction,
                    seed: req.seed,
                });
                let _ = cmd_tx.send(AgentCommand::Narrate {
                    text: req.text,
                    source: req.source.unwrap_or_else(|| "AGENT".into()),
                    priority,
                    voice,
                });
                crate::app::request_repaint();
                write_json(&mut stream, 200, r#"{"ok":true}"#);
            }
            _ => write_json(&mut stream, 400, r#"{"error":"invalid narrate JSON"}"#),
        },
        ("POST", "/v1/annotate") => match serde_json::from_slice::<AnnotateReq>(&body) {
            Ok(req)
                if req.lat.is_finite()
                    && req.lon.is_finite()
                    && (-90.0..=90.0).contains(&req.lat)
                    && (-180.0..=180.0).contains(&req.lon) =>
            {
                let _ = cmd_tx.send(AgentCommand::Annotate {
                    location: GeoPoint {
                        lat: req.lat,
                        lon: req.lon,
                    },
                    label: req.label,
                    ttl_secs: req.ttl_secs.unwrap_or(180),
                });
                crate::app::request_repaint();
                write_json(&mut stream, 200, r#"{"ok":true}"#);
            }
            _ => write_json(&mut stream, 400, r#"{"error":"invalid annotate JSON"}"#),
        },
        ("POST", "/v1/annotations/clear") => {
            let _ = cmd_tx.send(AgentCommand::ClearAnnotations);
            crate::app::request_repaint();
            write_json(&mut stream, 200, r#"{"ok":true}"#);
        }
        ("POST", "/v1/suggest-stop") => match serde_json::from_slice::<SuggestStopReq>(&body) {
            Ok(req)
                if req.lat.is_finite()
                    && req.lon.is_finite()
                    && (-90.0..=90.0).contains(&req.lat)
                    && (-180.0..=180.0).contains(&req.lon) =>
            {
                let _ = cmd_tx.send(AgentCommand::SuggestStop {
                    location: GeoPoint {
                        lat: req.lat,
                        lon: req.lon,
                    },
                    title: req.title.unwrap_or_else(|| "Agent Suggested Stop".into()),
                });
                crate::app::request_repaint();
                write_json(&mut stream, 200, r#"{"ok":true}"#);
            }
            _ => write_json(&mut stream, 400, r#"{"error":"invalid suggest-stop JSON"}"#),
        },
        _ => write_json(&mut stream, 404, r#"{"error":"not found"}"#),
    }
}

fn write_json(stream: &mut TcpStream, status: u16, json: &str) {
    write_bytes(stream, status, "application/json", json.as_bytes());
}

fn write_bytes(stream: &mut TcpStream, status: u16, content_type: &str, body: &[u8]) {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        503 => "Service Unavailable",
        _ => "Error",
    };
    let header = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_harness_controls_camera_layers_events_and_annotations() {
        let srv = start_loopback_server(0).expect("binds ephemeral loopback port");
        let port = srv.port;
        let mut harness = AgentHarness {
            server: Some(srv),
            last_event_id: None,
            last_event_count: 0,
            active_flight: None,
            active_orbit: None,
            status: format!("listening on 127.0.0.1:{port}"),
        };
        let mut model = AppModel::new();
        let mut narrator = Narrator::default();
        harness.publish_and_drain(&mut model, &mut narrator);

        let client = reqwest::blocking::Client::new();
        let state_resp = client
            .get(format!("http://127.0.0.1:{port}/v1/state"))
            .send()
            .unwrap();
        assert_eq!(state_resp.status(), 200);
        let state_body: serde_json::Value =
            serde_json::from_str(&state_resp.text().unwrap()).unwrap();
        assert_eq!(state_body["body"], "earth");

        // Camera control: switch to local mode at Zurich with instant duration
        let cam_resp = client
            .post(format!("http://127.0.0.1:{port}/v1/camera"))
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(
                serde_json::json!({
                    "lat": 47.37,
                    "lon": 8.54,
                    "mode": "local",
                    "zoom": 12.0,
                    "local_pitch": 0.8,
                    "duration_secs": 0.0
                })
                .to_string(),
            )
            .send()
            .unwrap();
        assert_eq!(cam_resp.status(), 200);

        // Layer control: Atlas theme, ridgelines, viewshed, flights
        let layers_resp = client
            .post(format!("http://127.0.0.1:{port}/v1/layers"))
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(
                serde_json::json!({
                    "theme": "Atlas",
                    "terrain_analysis": "ridgelines",
                    "viewshed": true,
                    "flights": true,
                    "tanaka_contours": true
                })
                .to_string(),
            )
            .send()
            .unwrap();
        assert_eq!(layers_resp.status(), 200);

        let ann_resp = client
            .post(format!("http://127.0.0.1:{port}/v1/annotate"))
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(
                serde_json::json!({
                    "lat": 47.37,
                    "lon": 8.54,
                    "label": "Agent Watchpoint",
                    "ttl_secs": 120
                })
                .to_string(),
            )
            .send()
            .unwrap();
        assert_eq!(ann_resp.status(), 200);

        harness.publish_and_drain(&mut model, &mut narrator);
        assert!(model.globe_view.local_mode);
        assert!((model.globe_view.local_center.lat - 47.37).abs() < 0.01);
        assert!((model.globe_view.local_zoom - 12.0).abs() < 0.01);
        assert_eq!(model.map_theme, MapTheme::Atlas);
        assert_eq!(model.terrain_analysis_mode, TerrainAnalysisMode::Ridgeline);
        assert!(model.viewshed.enabled);
        assert!(model.show_flights);
        assert!(model.tanaka_contours);
        assert_eq!(model.agent_annotations.len(), 1);
        assert_eq!(model.agent_annotations[0].label, "Agent Watchpoint");
        harness.stop();
    }
}
