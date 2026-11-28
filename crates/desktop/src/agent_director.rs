//! Built-in Pure-Rust Autonomous Local AI Agent Director.
//!
//! Talks directly to any local OpenAI-compatible LLM server (such as `ingine-serve`
//! on `http://127.0.0.1:8003/v1`, Ollama, llama.cpp, or vLLM) with zero Python or
//! external sidecars. Each cycle observes the live map pose, active layers, events,
//! convergences, nearby webcams, and an optional live viewport JPEG screenshot, then
//! decides where to pilot the camera, which layers/themes/tools to toggle, what to
//! annotate, and what spoken broadcast update to deliver in its own configured voice.

use crate::agent_harness::{
    AgentCommand, AgentHarness, CameraControl, CameraDto, EventDto, LayerControl, StateSnapshot,
    build_camera_dtos, build_event_dtos, build_state_snapshot,
};
use crate::model::{AppModel, GeoPoint};
use crate::narrator::{AgentVoiceSettings, Narrator, SpeechPriority, VoiceOverride};
use crossbeam_channel::{Receiver, Sender, unbounded};
use serde::Deserialize;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
struct DirectorConfig {
    llm_url: String,
    model: String,
    api_key: String,
    persona_name: String,
    system_prompt: String,
    voice_override: Option<VoiceOverride>,
}

impl DirectorConfig {
    fn from_settings(s: &AgentVoiceSettings) -> Self {
        Self {
            llm_url: s.director_llm_url.trim().trim_end_matches('/').to_owned(),
            model: s.director_model.trim().to_owned(),
            api_key: s.director_api_key.trim().to_owned(),
            persona_name: if s.director_persona_name.trim().is_empty() {
                "ATLAS-1".into()
            } else {
                s.director_persona_name.trim().to_owned()
            },
            system_prompt: s.director_system_prompt.trim().to_owned(),
            voice_override: s.director_voice_override(),
        }
    }
}

enum WorkerRequest {
    Step {
        config: DirectorConfig,
        state: StateSnapshot,
        events: Vec<EventDto>,
        cameras: Vec<CameraDto>,
        screenshot_jpeg: Option<Vec<u8>>,
        operator_nudge: Option<String>,
    },
    DetectModels {
        llm_url: String,
        api_key: String,
    },
    ResetMemory,
    Shutdown,
}

enum WorkerEvent {
    Status(String),
    Decision {
        thought: String,
        summary: String,
        commands: Vec<AgentCommand>,
        next_wait_secs: Option<u32>,
    },
    Error(String),
    DetectedModels {
        models: Vec<String>,
        error: Option<String>,
    },
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct DecisionAnnotation {
    lat: f32,
    lon: f32,
    label: String,
    ttl_secs: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct DirectorDecision {
    thought: Option<String>,
    camera: Option<CameraControl>,
    layers: Option<LayerControl>,
    annotations: Option<Vec<DecisionAnnotation>>,
    clear_annotations: Option<bool>,
    narrate: Option<String>,
    next_wait_secs: Option<u32>,
}

#[derive(Clone, Debug)]
struct MemoryEntry {
    turn: usize,
    summary: String,
    narrated: String,
    event_id: Option<String>,
    lat: Option<f32>,
    lon: Option<f32>,
}

pub struct AgentDirector {
    req_tx: Sender<WorkerRequest>,
    evt_rx: Receiver<WorkerEvent>,
    busy: bool,
    was_enabled: bool,
    awaiting_screenshot: bool,
    screenshot_requested_at: Option<Instant>,
    pending_nudge: Option<String>,
    next_cycle_at: Option<Instant>,
    pub status: String,
    pub last_thought: String,
    pub last_action: String,
    pub detected_models: Vec<String>,
}

impl Default for AgentDirector {
    fn default() -> Self {
        let (req_tx, req_rx) = unbounded::<WorkerRequest>();
        let (evt_tx, evt_rx) = unbounded::<WorkerEvent>();
        let _ = std::thread::Builder::new()
            .name("1kee-agent-director".into())
            .spawn(move || run_director_worker(req_rx, evt_tx));
        Self {
            req_tx,
            evt_rx,
            busy: false,
            was_enabled: false,
            awaiting_screenshot: false,
            screenshot_requested_at: None,
            pending_nudge: None,
            next_cycle_at: None,
            status: "Standby".into(),
            last_thought: String::new(),
            last_action: String::new(),
            detected_models: Vec::new(),
        }
    }
}

impl Drop for AgentDirector {
    fn drop(&mut self) {
        let _ = self.req_tx.send(WorkerRequest::Shutdown);
    }
}

impl AgentDirector {
    /// True when the director needs a fresh viewport screenshot on this frame.
    pub fn wants_screenshot(&self) -> bool {
        self.awaiting_screenshot
    }

    /// Feed a freshly captured viewport JPEG screenshot into a pending director step.
    pub fn deliver_screenshot(&mut self, jpeg: Vec<u8>, model: &AppModel) {
        if !self.awaiting_screenshot {
            return;
        }
        self.awaiting_screenshot = false;
        self.screenshot_requested_at = None;
        self.dispatch_step(model, Some(jpeg));
    }

    /// Trigger an immediate single autonomous turn (works even if continuous loop is off).
    pub fn trigger_step_now(&mut self, model: &AppModel, nudge: Option<String>) {
        if let Some(text) = nudge.filter(|s| !s.trim().is_empty()) {
            self.pending_nudge = Some(text.trim().to_owned());
        }
        if self.busy {
            return;
        }
        self.begin_step(model);
    }

    /// Query `{llm_url}/models` in the background to populate available model IDs.
    pub fn detect_models(&mut self, settings: &AgentVoiceSettings) {
        self.status = "Querying /v1/models…".into();
        let _ = self.req_tx.send(WorkerRequest::DetectModels {
            llm_url: settings.director_llm_url.trim().trim_end_matches('/').to_owned(),
            api_key: settings.director_api_key.trim().to_owned(),
        });
    }

    pub fn reset_memory(&mut self) {
        let _ = self.req_tx.send(WorkerRequest::ResetMemory);
        self.last_thought.clear();
        self.last_action.clear();
    }

    /// Advance the director state machine on every UI tick.
    pub fn tick(
        &mut self,
        model: &mut AppModel,
        harness: &mut AgentHarness,
        narrator: &mut Narrator,
        radio_playing: bool,
    ) {
        // Handle one-shot UI triggers from Settings / top rail.
        if model.pending_director_detect {
            model.pending_director_detect = false;
            self.detect_models(&model.agent_voice);
        }
        if let Some(nudge) = model.pending_director_nudge.take() {
            self.trigger_step_now(model, Some(nudge));
        } else if model.pending_director_step {
            model.pending_director_step = false;
            self.trigger_step_now(model, None);
        }

        // Drain completed worker events.
        while let Ok(evt) = self.evt_rx.try_recv() {
            match evt {
                WorkerEvent::Status(s) => {
                    self.status = s;
                }
                WorkerEvent::Decision {
                    thought,
                    summary,
                    commands,
                    next_wait_secs,
                } => {
                    self.busy = false;
                    self.last_thought = thought;
                    self.last_action = summary.clone();
                    for cmd in commands {
                        harness.apply_command(cmd, model, narrator);
                    }
                    let max_wait = model.agent_voice.director_interval_secs.clamp(8, 300);
                    let wait = next_wait_secs
                        .unwrap_or(max_wait)
                        .clamp(8, max_wait);
                    self.next_cycle_at = Some(Instant::now() + Duration::from_secs(u64::from(wait)));
                    self.status = format!("Active · next update in {wait}s");
                    model.push_log(format!(
                        "{}: {summary}",
                        model.agent_voice.director_persona_name
                    ));
                }
                WorkerEvent::Error(err) => {
                    self.busy = false;
                    self.status = format!("Error: {err}");
                    self.next_cycle_at = Some(Instant::now() + Duration::from_secs(15));
                    model.push_log(format!("Agent Director error: {err}"));
                }
                WorkerEvent::DetectedModels { models, error } => {
                    if let Some(err) = error {
                        self.status = format!("Model check failed: {err}");
                    } else if models.is_empty() {
                        self.status = "Connected, but /v1/models returned empty list".into();
                    } else {
                        if model.agent_voice.director_model.trim().is_empty()
                            || !models.iter().any(|m| m == model.agent_voice.director_model.trim())
                        {
                            model.agent_voice.director_model = models[0].clone();
                        }
                        self.status = format!("Connected · {}", models[0]);
                        self.detected_models = models;
                    }
                }
            }
        }

        // If waiting for a viewport screenshot timed out (>1.5s), proceed without image.
        if self.awaiting_screenshot
            && self
                .screenshot_requested_at
                .is_some_and(|t| t.elapsed() > Duration::from_millis(1500))
        {
            self.awaiting_screenshot = false;
            self.screenshot_requested_at = None;
            self.dispatch_step(model, None);
        }

        // Continuous autonomous loop when enabled.
        if !model.agent_voice.director_enabled {
            if self.was_enabled {
                harness.cancel_flight();
            }
            self.was_enabled = false;
            self.next_cycle_at = None;
            if !self.busy && self.status.starts_with("Active") {
                self.status = "Standby".into();
            }
        } else {
            self.was_enabled = true;
            // Keep Follow Events / Meander off so the AI Agent owns the camera.
            if model.event_follow.enabled() {
                model.stop_event_follow();
            }
            if model.globe_view.meander_mode {
                model.globe_view.meander_mode = false;
            }
            let radio_blocks = model.agent_voice.yield_to_radio && radio_playing;
            if !self.busy && !self.awaiting_screenshot && !narrator.is_busy() && !radio_blocks {
                let due = self
                    .next_cycle_at
                    .map_or(true, |deadline| Instant::now() >= deadline);
                if due {
                    self.begin_step(model);
                }
            }
        }

        // Mirror status & last thought/action to AppModel for Settings UI.
        model.agent_director_status = self.status.clone();
        model.agent_director_last_action = self.last_action.clone();
        model.agent_director_last_thought = self.last_thought.clone();
        model.agent_director_detected_models = self.detected_models.clone();
    }

    fn begin_step(&mut self, model: &AppModel) {
        if model.agent_voice.director_send_screenshot {
            self.awaiting_screenshot = true;
            self.screenshot_requested_at = Some(Instant::now());
            self.status = "Capturing map view…".into();
            crate::app::request_repaint();
        } else {
            self.dispatch_step(model, None);
        }
    }

    fn dispatch_step(&mut self, model: &AppModel, screenshot_jpeg: Option<Vec<u8>>) {
        self.busy = true;
        self.status = format!("Thinking ({})…", model.agent_voice.director_model);
        let center = if model.globe_view.local_mode {
            model.globe_view.local_center
        } else {
            model.globe_view.globe_center_latlon()
        };
        let req = WorkerRequest::Step {
            config: DirectorConfig::from_settings(&model.agent_voice),
            state: build_state_snapshot(model),
            events: build_event_dtos(model, 16),
            cameras: build_camera_dtos(model, center, 10),
            screenshot_jpeg,
            operator_nudge: self.pending_nudge.take(),
        };
        let _ = self.req_tx.send(req);
        crate::app::request_repaint();
    }
}

fn run_director_worker(rx: Receiver<WorkerRequest>, tx: Sender<WorkerEvent>) {
    let client = match reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(90))
        .build()
    {
        Ok(c) => c,
        Err(err) => {
            let _ = tx.send(WorkerEvent::Error(format!("HTTP client init failed: {err}")));
            return;
        }
    };

    let mut memory: VecDeque<MemoryEntry> = VecDeque::with_capacity(12);
    let mut turn_counter: usize = 0;
    let mut skip_vision_due_to_oom = false;

    while let Ok(req) = rx.recv() {
        match req {
            WorkerRequest::Shutdown => break,
            WorkerRequest::ResetMemory => {
                memory.clear();
                turn_counter = 0;
                skip_vision_due_to_oom = false;
            }
            WorkerRequest::DetectModels { llm_url, api_key } => {
                match fetch_models(&client, &llm_url, &api_key) {
                    Ok(models) => {
                        let _ = tx.send(WorkerEvent::DetectedModels {
                            models,
                            error: None,
                        });
                    }
                    Err(err) => {
                        let _ = tx.send(WorkerEvent::DetectedModels {
                            models: Vec::new(),
                            error: Some(err),
                        });
                    }
                }
                crate::app::request_repaint();
            }
            WorkerRequest::Step {
                config,
                state,
                events,
                cameras,
                screenshot_jpeg,
                operator_nudge,
            } => {
                let _ = tx.send(WorkerEvent::Status(format!(
                    "Analyzing map & events ({})…",
                    config.model
                )));
                crate::app::request_repaint();
                let effective_shot = if skip_vision_due_to_oom {
                    None
                } else {
                    screenshot_jpeg.as_deref()
                };
                match execute_director_turn(
                    &client,
                    &config,
                    &state,
                    &events,
                    &cameras,
                    effective_shot,
                    operator_nudge.as_deref(),
                    &memory,
                    &mut skip_vision_due_to_oom,
                ) {
                    Ok(decision) => {
                        turn_counter += 1;
                        let (commands, summary, narrated, event_id, focus_coord) =
                            decision_to_commands(decision, &config);
                        let thought = summary.0;
                        let action_desc = summary.1;
                        let next_wait = summary.2;
                        let (lat, lon) = focus_coord.unwrap_or((state.center_lat, state.center_lon));
                        memory.push_back(MemoryEntry {
                            turn: turn_counter,
                            summary: action_desc.clone(),
                            narrated,
                            event_id,
                            lat: Some(lat),
                            lon: Some(lon),
                        });
                        while memory.len() > 10 {
                            memory.pop_front();
                        }
                        let _ = tx.send(WorkerEvent::Decision {
                            thought,
                            summary: action_desc,
                            commands,
                            next_wait_secs: next_wait,
                        });
                    }
                    Err(err) => {
                        let _ = tx.send(WorkerEvent::Error(err));
                    }
                }
                crate::app::request_repaint();
            }
        }
    }
}

fn fetch_models(
    client: &reqwest::blocking::Client,
    llm_url: &str,
    api_key: &str,
) -> Result<Vec<String>, String> {
    let url = format!("{}/models", llm_url.trim_end_matches('/'));
    let mut req = client.get(&url);
    if !api_key.is_empty() {
        req = req.bearer_auth(api_key);
    }
    let resp = req.send().map_err(|e| format!("{e}"))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let text = resp.text().map_err(|e| format!("Read error: {e}"))?;
    let json: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("Invalid JSON: {e}"))?;
    let models = json["data"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|item| item["id"].as_str().map(str::to_owned))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Ok(models)
}

#[allow(clippy::too_many_arguments)]
fn execute_director_turn(
    client: &reqwest::blocking::Client,
    config: &DirectorConfig,
    state: &StateSnapshot,
    events: &[EventDto],
    cameras: &[CameraDto],
    screenshot_jpeg: Option<&[u8]>,
    operator_nudge: Option<&str>,
    memory: &VecDeque<MemoryEntry>,
    skip_vision_due_to_oom: &mut bool,
) -> Result<DirectorDecision, String> {
    let system_prompt = build_system_prompt(config);
    let user_text = build_observation_prompt(state, events, cameras, operator_nudge, memory);

    let result = call_chat_completions(client, config, &system_prompt, &user_text, screenshot_jpeg);
    let raw_reply = match result {
        Ok(r) => r,
        Err(err)
            if screenshot_jpeg.is_some()
                && (err.contains("HTTP 400")
                    || err.contains("out of memory")
                    || err.contains("cudaMalloc")
                    || err.contains("HTTP 500")) =>
        {
            // Automatic fallback to text-only if the local model rejects images or
            // hits GPU VRAM limits during multimodal vision encoding.
            if err.contains("out of memory") || err.contains("cudaMalloc") {
                *skip_vision_due_to_oom = true;
            }
            call_chat_completions(client, config, &system_prompt, &user_text, None)?
        }
        Err(err) => return Err(err),
    };

    parse_director_decision(&raw_reply)
}

fn build_system_prompt(config: &DirectorConfig) -> String {
    format!(
        "{}\n\n\
        ## Output Protocol (STRICT)\n\
        Respond with ONLY a single valid JSON object (no markdown commentary outside the JSON). \
        You MUST include a spoken `narrate` update on EVERY SINGLE TURN. \
        Other fields are optional — include what you want to steer or toggle this turn:\n\
        {{\n\
          \"thought\": \"1-sentence editorial reasoning\",\n\
          \"camera\": {{\n\
            \"lat\": 46.5, \"lon\": 7.9,\n\
            \"mode\": \"globe\" | \"local\",\n\
            \"body\": \"earth\" | \"moon\" | \"mars\",\n\
            \"zoom\": 2.5,\n\
            \"local_pitch\": 0.9, \"local_yaw\": -0.5, \"layer_spread\": 2.0,\n\
            \"duration_secs\": 2.2,\n\
            \"select_event_id\": \"event-id-if-focusing-an-event\",\n\
            \"open_camera_id\": \"camera-id-if-opening-live-feed\",\n\
            \"close_camera\": false\n\
          }},\n\
          \"layers\": {{\n\
            \"theme\": \"Atlas\" | \"Topo\" | \"Abyss\" | \"Phosphor\" | \"Thermal\" | \"Ghost\" | \"Akira\" | \"Sodium\",\n\
            \"terrain_analysis\": \"off\" | \"slope\" | \"aspect\" | \"ridgelines\",\n\
            \"viewshed\": true|false, \"viewshed_follow_center\": true,\n\
            \"tanaka_contours\": true|false, \"contours\": true|false,\n\
            \"bathymetry\": true|false, \"flights\": true|false, \"ships\": true|false,\n\
            \"fires\": true|false, \"webcams\": true|false, \"roads\": true|false,\n\
            \"buildings\": true|false, \"power\": true|false, \"military\": true|false,\n\
            \"submarine_cables\": true|false, \"atlas_light_mode\": \"studio\"|\"sun\"\n\
          }},\n\
          \"annotations\": [{{\"lat\": 46.5, \"lon\": 7.9, \"label\": \"Short label\", \"ttl_secs\": 180}}],\n\
          \"clear_annotations\": false,\n\
          \"narrate\": \"REQUIRED every turn: 2 to 4 engaging, natural spoken sentences for live audio broadcast explaining what you are showing, what the terrain/events/cameras reveal, and why it matters. NEVER include raw lat/lon coordinates or the word Factal.\",\n\
          \"next_wait_secs\": 14\n\
        }}\n\
        Broadcast Pacing Rules:\n\
        - ALWAYS speak in `narrate` every turn — never go silent while exploring.\n\
        - After spending 1–2 turns on a story or checking a webcam, fly on to a NEW active event, convergence, or region (and set `close_camera: true` when moving on) so the broadcast stays lively and covers the whole globe.\n\
        - The camera automatically orbits your chosen point of focus once you arrive, so switching between Globe overviews (zoom 1.8–4.5) and Local 3D Terrain close-ups (zoom 5.0–14.0 with Atlas relief, Viewshed, or Ridgelines) looks great.",
        config.system_prompt
    )
}

fn build_observation_prompt(
    state: &StateSnapshot,
    events: &[EventDto],
    cameras: &[CameraDto],
    operator_nudge: Option<&str>,
    memory: &VecDeque<MemoryEntry>,
) -> String {
    let mut out = String::with_capacity(4096);
    out.push_str("## Live 1kEE Map State\n");
    out.push_str(&format!(
        "UTC: {} | Body: {} | Mode: {} | Center: ({:.3}, {:.3}) | Zoom: {:.2} | Theme: {} | Analysis: {}\n",
        state.utc,
        state.body,
        if state.local_mode { "local" } else { "globe" },
        state.center_lat,
        state.center_lon,
        state.zoom,
        state.theme,
        state.terrain_analysis,
    ));
    out.push_str(&format!(
        "Counts: {} events, {} webcams, {} flights, {} ships, {} active fires\n",
        state.event_count,
        state.camera_count,
        state.flight_count,
        state.ship_count,
        state.fire_count,
    ));
    out.push_str(&format!(
        "Active layers: contours={}, tanaka={}, viewshed={}, bathymetry={}, flights={}, ships={}, fires={}, webcams={}, roads={}, buildings={}, cables={}\n",
        state.layers.contours,
        state.layers.tanaka_contours,
        state.layers.viewshed,
        state.layers.bathymetry,
        state.layers.flights,
        state.layers.ships,
        state.layers.fires,
        state.layers.webcams,
        state.layers.roads,
        state.layers.buildings,
        state.layers.submarine_cables,
    ));

    if !state.convergences.is_empty() {
        out.push_str("\n## Top Multi-Sensor Convergences\n");
        for c in &state.convergences {
            out.push_str(&format!(
                "- [{}] score={:.1} ({:.2}, {:.2}): {}\n",
                c.id, c.score, c.lat, c.lon, c.title
            ));
        }
    }

    if !events.is_empty() {
        out.push_str("\n## Active Live Events\n");
        for ev in events.iter().take(12) {
            let summary_clip: String = ev.summary.chars().take(160).collect();
            out.push_str(&format!(
                "- id=\"{}\" [{}] {} ({:.2}, {:.2}) | {} — {}\n",
                ev.id, ev.severity, ev.location_name, ev.lat, ev.lon, ev.title, summary_clip
            ));
        }
    }

    if !cameras.is_empty() {
        out.push_str("\n## Nearest Live Webcams to Current Focus\n");
        for cam in cameras.iter().take(6) {
            out.push_str(&format!(
                "- id=\"{}\" ({:.1} km away): {} [{}]\n",
                cam.id, cam.distance_km, cam.label, cam.provider
            ));
        }
    }

    if !memory.is_empty() {
        out.push_str("\n## Your Recent Turns (Do NOT repeat these — progress the coverage)\n");
        for entry in memory {
            let ev_tag = entry
                .event_id
                .as_deref()
                .map(|id| format!(" [event={id}]"))
                .unwrap_or_default();
            out.push_str(&format!(
                "- Turn {}{}: {} | Said: \"{}\"\n",
                entry.turn, ev_tag, entry.summary, entry.narrated
            ));
        }
        // Count how many consecutive trailing turns stayed near the current focus.
        let mut consecutive_same_spot = 0usize;
        for entry in memory.iter().rev() {
            if let (Some(lat), Some(lon)) = (entry.lat, entry.lon)
                && (lat - state.center_lat).abs() < 3.5
                && (lon - state.center_lon).abs() < 3.5
            {
                consecutive_same_spot += 1;
            } else {
                break;
            }
        }
        if consecutive_same_spot >= 2 {
            out.push_str(&format!(
                "\n## PACING DIRECTIVE\nYou have stayed in this same area for {consecutive_same_spot} consecutive turns. \
                 Fly the camera to a DIFFERENT active event, convergence, or region on the globe THIS turn \
                 (set `close_camera: true` if a webcam is open) and deliver a fresh spoken update!\n"
            ));
        }
    }

    if let Some(nudge) = operator_nudge {
        out.push_str(&format!(
            "\n## DIRECT OPERATOR INSTRUCTION (PRIORITIZE THIS NOW)\n{nudge}\n"
        ));
    }

    out.push_str("\nChoose your next camera move, layer adjustments, annotations, and spoken update as a single JSON object.");
    out
}

fn call_chat_completions(
    client: &reqwest::blocking::Client,
    config: &DirectorConfig,
    system_prompt: &str,
    user_text: &str,
    screenshot_jpeg: Option<&[u8]>,
) -> Result<String, String> {
    let url = format!("{}/chat/completions", config.llm_url.trim_end_matches('/'));
    let user_content = if let Some(jpeg) = screenshot_jpeg {
        let data_url = format!("data:image/jpeg;base64,{}", encode_base64(jpeg));
        serde_json::json!([
            { "type": "text", "text": user_text },
            { "type": "image_url", "image_url": { "url": data_url } }
        ])
    } else {
        serde_json::Value::String(user_text.to_owned())
    };

    let payload = serde_json::json!({
        "model": config.model,
        "messages": [
            { "role": "system", "content": system_prompt },
            { "role": "user", "content": user_content }
        ],
        "max_tokens": 2048,
        "temperature": 0.45
    });

    let mut req = client
        .post(&url)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(payload.to_string());
    if !config.api_key.is_empty() {
        req = req.bearer_auth(&config.api_key);
    }

    let resp = req
        .send()
        .map_err(|e| format!("LLM request failed ({url}): {e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        let clip: String = body.trim().chars().take(180).collect();
        return Err(format!("LLM HTTP {status}: {clip}"));
    }

    let text = resp
        .text()
        .map_err(|e| format!("Failed reading LLM response body: {e}"))?;
    let json: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("LLM returned invalid JSON envelope: {e}"))?;
    let msg = &json["choices"][0]["message"];
    let content = msg["content"].as_str().unwrap_or("").trim();
    if !content.is_empty() {
        return Ok(content.to_owned());
    }
    // Fallback if a reasoning model ran close to token budget and placed JSON in reasoning_content.
    let reasoning = msg["reasoning_content"].as_str().unwrap_or("").trim();
    if !reasoning.is_empty() {
        return Ok(reasoning.to_owned());
    }
    Err("LLM returned an empty assistant message".into())
}

pub fn parse_director_decision(raw: &str) -> Result<DirectorDecision, String> {
    let trimmed = raw.trim();
    if let Ok(d) = serde_json::from_str::<DirectorDecision>(trimmed) {
        return Ok(d);
    }
    // Strip ```json ... ``` fences if present.
    if let Some(start_fence) = trimmed.find("```") {
        let after_fence = &trimmed[start_fence + 3..];
        let body = after_fence
            .strip_prefix("json")
            .or_else(|| after_fence.strip_prefix("JSON"))
            .unwrap_or(after_fence);
        if let Some(end_fence) = body.find("```") {
            let candidate = body[..end_fence].trim();
            if let Ok(d) = serde_json::from_str::<DirectorDecision>(candidate) {
                return Ok(d);
            }
        }
    }
    // Extract the outermost balanced `{ ... }` JSON object.
    if let Some(obj) = extract_json_object(trimmed)
        && let Ok(d) = serde_json::from_str::<DirectorDecision>(obj)
    {
        return Ok(d);
    }
    let preview: String = trimmed.chars().take(140).collect();
    Err(format!("Could not parse director JSON: {preview}"))
}

fn extract_json_object(s: &str) -> Option<&str> {
    let bytes = s.as_bytes();
    let mut start = None;
    let mut depth = 0usize;
    let mut in_str = false;
    let mut escape = false;
    for (i, &b) in bytes.iter().enumerate() {
        if in_str {
            if escape {
                escape = false;
            } else if b == b'\\' {
                escape = true;
            } else if b == b'"' {
                in_str = false;
            }
            continue;
        }
        match b {
            b'"' => in_str = true,
            b'{' => {
                if start.is_none() {
                    start = Some(i);
                }
                depth += 1;
            }
            b'}' => {
                if depth > 0 {
                    depth -= 1;
                    if depth == 0
                        && let Some(st) = start
                    {
                        return Some(&s[st..=i]);
                    }
                }
            }
            _ => {}
        }
    }
    None
}

fn decision_to_commands(
    decision: DirectorDecision,
    config: &DirectorConfig,
) -> (
    Vec<AgentCommand>,
    (String, String, Option<u32>),
    String,
    Option<String>,
    Option<(f32, f32)>,
) {
    let mut cmds = Vec::new();
    let mut parts = Vec::new();
    let mut event_id = None;
    let mut focus_coord = None;

    if decision.clear_annotations == Some(true) {
        cmds.push(AgentCommand::ClearAnnotations);
    }
    if let Some(layers) = decision.layers {
        if let Some(theme) = &layers.theme {
            parts.push(format!("theme={theme}"));
        }
        if let Some(an) = &layers.terrain_analysis {
            parts.push(format!("analysis={an}"));
        }
        if layers.viewshed == Some(true) {
            parts.push("viewshed=on".into());
        }
        cmds.push(AgentCommand::Layers(layers));
    }
    if let Some(cam) = decision.camera {
        event_id = cam.select_event_id.clone();
        if let (Some(lat), Some(lon)) = (cam.lat, cam.lon) {
            focus_coord = Some((lat, lon));
            let mode = cam.mode.as_deref().unwrap_or("view");
            parts.push(format!("fly({mode} {lat:.2}°, {lon:.2}°)"));
        } else if let Some(mode) = &cam.mode {
            parts.push(format!("mode={mode}"));
        }
        cmds.push(AgentCommand::Camera(cam));
    }
    if let Some(anns) = decision.annotations {
        for ann in anns.into_iter().take(4) {
            if ann.lat.is_finite()
                && ann.lon.is_finite()
                && (-90.0..=90.0).contains(&ann.lat)
                && (-180.0..=180.0).contains(&ann.lon)
                && !ann.label.trim().is_empty()
            {
                parts.push(format!("pin '{}'", ann.label.trim()));
                cmds.push(AgentCommand::Annotate {
                    location: GeoPoint {
                        lat: ann.lat,
                        lon: ann.lon,
                    },
                    label: ann.label.trim().to_owned(),
                    ttl_secs: ann.ttl_secs.unwrap_or(180),
                });
            }
        }
    }
    let mut narrated = String::new();
    if let Some(text) = decision.narrate {
        let clean = crate::narrator::sanitize_for_speech(&text);
        if !clean.is_empty() {
            narrated = clean.clone();
            cmds.push(AgentCommand::Narrate {
                text: clean,
                source: config.persona_name.clone(),
                priority: SpeechPriority::Normal,
                voice: config.voice_override.clone(),
            });
        }
    }

    let thought = decision.thought.unwrap_or_default().trim().to_owned();
    let summary = if parts.is_empty() {
        if !narrated.is_empty() {
            "Spoken update".into()
        } else {
            "Holding view".into()
        }
    } else {
        parts.join(" · ")
    };

    (
        cmds,
        (thought, summary, decision.next_wait_secs),
        narrated,
        event_id,
        focus_coord,
    )
}

/// Downscale an `egui::ColorImage` screenshot to at most `512 px` wide and encode as JPEG
/// so multimodal vision encoding uses minimal GPU VRAM alongside large local LLMs.
pub fn encode_viewport_screenshot_jpeg(img: &egui::ColorImage) -> Option<Vec<u8>> {
    let [w, h] = img.size;
    if w == 0 || h == 0 {
        return None;
    }
    let mut rgb = Vec::with_capacity(w * h * 3);
    for px in &img.pixels {
        rgb.push(px.r());
        rgb.push(px.g());
        rgb.push(px.b());
    }
    let buf = image::RgbImage::from_raw(w as u32, h as u32, rgb)?;
    let dyn_img = image::DynamicImage::ImageRgb8(buf);
    let resized = if w > 512 {
        let new_h = ((h as f32) * (512.0 / w as f32)).round().max(1.0) as u32;
        dyn_img.resize_exact(512, new_h, image::imageops::FilterType::Triangle)
    } else {
        dyn_img
    };
    let mut out = Vec::with_capacity(32 * 1024);
    let mut cursor = std::io::Cursor::new(&mut out);
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut cursor, 72);
    enc.encode_image(&resized).ok()?;
    Some(out)
}

fn encode_base64(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    let mut i = 0;
    while i < input.len() {
        let b0 = input[i];
        let b1 = if i + 1 < input.len() { input[i + 1] } else { 0 };
        let b2 = if i + 2 < input.len() { input[i + 2] } else { 0 };
        let n = ((b0 as u32) << 16) | ((b1 as u32) << 8) | (b2 as u32);
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        out.push(if i + 1 < input.len() {
            TABLE[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if i + 2 < input.len() {
            TABLE[(n & 63) as usize] as char
        } else {
            '='
        });
        i += 3;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fenced_and_embedded_director_json() {
        let raw = "Sure, here is the plan:\n```json\n{\"thought\":\"Zoom to Alps\",\"camera\":{\"lat\":46.5,\"lon\":7.9,\"mode\":\"local\",\"zoom\":10.0},\"layers\":{\"theme\":\"Atlas\",\"viewshed\":true},\"narrate\":\"Inspecting the Swiss Alps in 3D relief.\",\"next_wait_secs\":30}\n```";
        let d = parse_director_decision(raw).expect("parses fenced JSON");
        assert_eq!(d.thought.as_deref(), Some("Zoom to Alps"));
        assert_eq!(d.camera.as_ref().unwrap().mode.as_deref(), Some("local"));
        assert_eq!(d.layers.as_ref().unwrap().theme.as_deref(), Some("Atlas"));
        assert_eq!(d.next_wait_secs, Some(30));
    }

    #[test]
    fn base64_encoder_matches_standard_vectors() {
        assert_eq!(encode_base64(b""), "");
        assert_eq!(encode_base64(b"f"), "Zg==");
        assert_eq!(encode_base64(b"fo"), "Zm8=");
        assert_eq!(encode_base64(b"foo"), "Zm9v");
        assert_eq!(encode_base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    #[ignore = "live integration test against local OpenAI-compatible LLM on 127.0.0.1:8003"]
    fn live_local_llm_produces_valid_map_control_decision() {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .unwrap();
        let settings = AgentVoiceSettings::default();
        let config = DirectorConfig::from_settings(&settings);
        let model = AppModel::new();
        let state = build_state_snapshot(&model);
        let events = vec![EventDto {
            id: "ev-alps-1".into(),
            title: "Severe avalanche warning across Bernese Oberland".into(),
            summary: "Heavy snowfall triggered multiple slab avalanches near Grindelwald and Lauterbrunnen.".into(),
            severity: "Elevated",
            location_name: "Grindelwald, Switzerland".into(),
            lat: 46.624,
            lon: 8.041,
            occurred_at: "2026-11-26T00:45:00Z".into(),
            convergence_score: Some(8.4),
        }];
        let mut skip_vision = false;
        let decision = execute_director_turn(
            &client,
            &config,
            &state,
            &events,
            &[],
            None,
            Some("Switch to Atlas mode, fly to the Alps event in local 3D mode, enable viewshed, and narrate a short update."),
            &VecDeque::new(),
            &mut skip_vision,
        )
        .expect("live LLM returned valid DirectorDecision JSON");
        assert!(decision.camera.is_some(), "expected camera command: {decision:?}");
        assert!(decision.narrate.as_deref().is_some_and(|s| !s.trim().is_empty()));
        println!("Live LLM decision: {decision:?}");
    }
}
