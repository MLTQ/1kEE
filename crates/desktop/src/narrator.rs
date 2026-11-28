//! Opt-in BreezeTTS Broadcast Narrator & OBS Caption Overlay.
//!
//! Strictly opt-in (disabled by default) and user-configured: never probes local
//! ports automatically. Supports native BreezeTTS multipart `/v1/audio/speech`
//! (streaming 24 kHz s16le mono PCM), OpenAI-compatible JSON `/v1/audio/speech`,
//! and simple `{ "text": ... }` JSON payloads, with a sequential priority speech
//! queue, rate-limited cooldowns, radio-yield protection, and an OBS caption pill.

use crate::model::AppModel;
use crate::theme;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

const DEFAULT_BREEZE_INSTRUCTION: &str =
    "A calm, clear broadcast narrator with natural pacing and crisp diction.";

pub const DEFAULT_DIRECTOR_SYSTEM_PROMPT: &str = "You are ATLAS-1, an autonomous live geospatial broadcast analyst piloting One Thousand Electric Eye (1kEE), a 3D Earth/Moon/Mars cartographic globe and terrain engine. You control the camera, map layers, themes, viewshed, annotations, and spoken audio commentary directly. Choose compelling active events, multi-sensor convergences, maritime/aviation chokepoints, volcanic/seismic corridors, or dramatic terrain features. Move smoothly between Globe overview and Local 3D Terrain close-ups, toggle relevant analytical layers (Atlas relief, Slope, Ridgelines, Viewshed, Flights, Ships, Active Fires, Bathymetry, Webcams), drop concise map annotations, and deliver crisp, insightful 1-3 sentence spoken updates as situations evolve. Never repeat what you just covered, never read raw latitude/longitude numbers aloud, and never mention the word Factal.";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TtsRequestFormat {
    #[default]
    EmbeddedReverb,
    BreezeMultipart,
    OpenAiSpeech,
    SimpleText,
}

impl TtsRequestFormat {
    pub fn label(self) -> &'static str {
        match self {
            Self::EmbeddedReverb => "Built-in ReverbTTS (Rust)",
            Self::BreezeMultipart => "BreezeTTS HTTP",
            Self::OpenAiSpeech => "OpenAI JSON",
            Self::SimpleText => "Simple JSON",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReverbDeviceChoice {
    #[default]
    Cuda0,
    Cuda1,
    Cpu,
}

impl ReverbDeviceChoice {
    pub fn label(self) -> &'static str {
        match self {
            Self::Cuda0 => "CUDA 0 (Fastest GPU)",
            Self::Cuda1 => "CUDA 1 (Secondary GPU)",
            Self::Cpu => "CPU",
        }
    }

    pub fn into_compute(self) -> reverb_tts::ComputeDevice {
        match self {
            Self::Cuda0 => reverb_tts::ComputeDevice::Cuda(0),
            Self::Cuda1 => reverb_tts::ComputeDevice::Cuda(1),
            Self::Cpu => reverb_tts::ComputeDevice::Cpu,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentVoiceSettings {
    /// Opt-in broadcast narration playback.
    pub tts_enabled: bool,
    /// Built-in ReverbTTS checkpoints directory (contains `Reverb-TTS/` & `higgs-audio-v2-tokenizer/`).
    pub reverb_model_dir: String,
    /// Built-in ReverbTTS preset voice name (from `voices/<name>.json`) or full path to a voice `.json`.
    pub reverb_voice_preset: String,
    /// Language code for ReverbTTS (`en`, `de`, `fr`, `es`, `it`, `pl`, `pt`).
    pub reverb_language: String,
    /// Compute backend for built-in ReverbTTS.
    pub reverb_device: ReverbDeviceChoice,
    /// Prefer compact INT4 Reverb-TTS checkpoint (`182 MB`) over FP16 (`567 MB`).
    pub reverb_prefer_int4: bool,
    /// User-configured local HTTP TTS endpoint URL (never auto-probed).
    pub tts_endpoint: String,
    pub tts_voice: String,
    pub tts_model: String,
    pub tts_format: TtsRequestFormat,
    /// Optional BreezeTTS voice-design instruction (used when no reference WAV is set).
    pub tts_instruction: String,
    /// Optional BreezeTTS voice-cloning reference WAV path.
    pub tts_ref_audio: String,
    /// Exact transcript matching `tts_ref_audio`.
    pub tts_ref_text: String,
    /// BreezeTTS classifier-free guidance scale (1.0 for ref WAV cloning, 4.0 for instruction).
    pub tts_cfg_scale: f32,
    /// Sampling seed (shared by ReverbTTS & BreezeTTS).
    pub tts_seed: i64,
    /// Automatically narrate new Follow Events stops & multi-sensor convergences.
    pub auto_narrate_follow: bool,
    /// Minimum wall seconds between automatic narrations.
    pub cooldown_secs: u32,
    /// Yield narration when Nearby Radio is actively playing.
    pub yield_to_radio: bool,
    /// Opt-in loopback-only (`127.0.0.1`) local agent HTTP/SSE bridge.
    pub agent_harness_enabled: bool,
    pub agent_harness_port: u16,
    // ── Autonomous Local AI Agent Director ────────────────────────────────────
    /// Opt-in built-in autonomous agent loop that pilots the camera, layers, and voice.
    pub director_enabled: bool,
    /// OpenAI-compatible local LLM base URL (e.g. `http://127.0.0.1:8003/v1`).
    pub director_llm_url: String,
    /// Model ID served by the local LLM endpoint.
    pub director_model: String,
    /// Optional Bearer API key (leave empty for local servers).
    pub director_api_key: String,
    /// Display name / callsign for the agent in captions and logs.
    pub director_persona_name: String,
    /// System prompt defining the agent's personality, priorities, and broadcast style.
    pub director_system_prompt: String,
    /// Seconds to pause between autonomous analysis & camera cycles.
    pub director_interval_secs: u32,
    /// Send a live viewport JPEG screenshot to multimodal LLMs so the agent can see the map.
    pub director_send_screenshot: bool,
    /// Optional dedicated preset voice for the agent (blank = inherit narrator voice).
    pub director_voice_preset: String,
    /// Optional dedicated reference WAV for the agent's own cloned voice.
    pub director_ref_audio: String,
    /// Exact spoken transcript matching `director_ref_audio`.
    pub director_ref_text: String,
    /// Language code for the agent's dedicated voice (`en`, `de`, `fr`, `es`, `it`, `pl`, `pt`).
    pub director_language: String,
}

impl Default for AgentVoiceSettings {
    fn default() -> Self {
        Self {
            tts_enabled: false,
            reverb_model_dir: "/run/media/m/Minkowski/Models/ReverbTTS".into(),
            reverb_voice_preset: "en_male_narrator_1".into(),
            reverb_language: "en".into(),
            reverb_device: ReverbDeviceChoice::Cuda0,
            reverb_prefer_int4: true,
            tts_endpoint: "http://127.0.0.1:7866/v1/audio/speech".into(),
            tts_voice: "default".into(),
            tts_model: "breeze".into(),
            tts_format: TtsRequestFormat::EmbeddedReverb,
            tts_instruction: DEFAULT_BREEZE_INSTRUCTION.into(),
            tts_ref_audio: String::new(),
            tts_ref_text: String::new(),
            tts_cfg_scale: 4.0,
            tts_seed: 42,
            auto_narrate_follow: false,
            cooldown_secs: 25,
            yield_to_radio: true,
            agent_harness_enabled: false,
            agent_harness_port: 18765,
            director_enabled: false,
            director_llm_url: "http://127.0.0.1:8003/v1".into(),
            director_model: "inkling-2-mini-abliterated".into(),
            director_api_key: String::new(),
            director_persona_name: "ATLAS-1".into(),
            director_system_prompt: DEFAULT_DIRECTOR_SYSTEM_PROMPT.into(),
            director_interval_secs: 14,
            director_send_screenshot: true,
            director_voice_preset: String::new(),
            director_ref_audio: String::new(),
            director_ref_text: String::new(),
            director_language: "en".into(),
        }
    }
}

impl AgentVoiceSettings {
    pub fn normalized(mut self) -> Self {
        self.reverb_model_dir = self.reverb_model_dir.trim().to_owned();
        self.reverb_voice_preset = self.reverb_voice_preset.trim().to_owned();
        if self.reverb_voice_preset.is_empty() {
            self.reverb_voice_preset = "en_male_narrator_1".into();
        }
        self.reverb_language = self.reverb_language.trim().to_owned();
        if !reverb_tts::text::SUPPORTED_LANGUAGES.contains(&self.reverb_language.as_str()) {
            self.reverb_language = "en".into();
        }
        self.tts_endpoint = self.tts_endpoint.trim().to_owned();
        self.tts_voice = self.tts_voice.trim().to_owned();
        self.tts_model = self.tts_model.trim().to_owned();
        self.tts_instruction = self.tts_instruction.trim().to_owned();
        self.tts_ref_audio = self.tts_ref_audio.trim().to_owned();
        self.tts_ref_text = self.tts_ref_text.trim().to_owned();
        if !self.tts_cfg_scale.is_finite() || self.tts_cfg_scale <= 0.0 {
            self.tts_cfg_scale = if self.tts_ref_audio.is_empty() { 4.0 } else { 1.0 };
        }
        self.tts_cfg_scale = self.tts_cfg_scale.clamp(0.1, 10.0);
        self.cooldown_secs = self.cooldown_secs.clamp(5, 300);
        if self.agent_harness_port < 1024 {
            self.agent_harness_port = 18765;
        }
        self.director_llm_url = self.director_llm_url.trim().trim_end_matches('/').to_owned();
        if self.director_llm_url.is_empty() {
            self.director_llm_url = "http://127.0.0.1:8003/v1".into();
        }
        self.director_model = self.director_model.trim().to_owned();
        self.director_api_key = self.director_api_key.trim().to_owned();
        self.director_persona_name = self.director_persona_name.trim().to_owned();
        if self.director_persona_name.is_empty() {
            self.director_persona_name = "ATLAS-1".into();
        }
        if self.director_system_prompt.trim().is_empty() {
            self.director_system_prompt = DEFAULT_DIRECTOR_SYSTEM_PROMPT.into();
        }
        self.director_interval_secs = self.director_interval_secs.clamp(8, 300);
        self.director_voice_preset = self.director_voice_preset.trim().to_owned();
        self.director_ref_audio = self.director_ref_audio.trim().to_owned();
        self.director_ref_text = self.director_ref_text.trim().to_owned();
        self.director_language = self.director_language.trim().to_owned();
        if !reverb_tts::text::SUPPORTED_LANGUAGES.contains(&self.director_language.as_str()) {
            self.director_language = "en".into();
        }
        self
    }

    /// Build a per-call voice override from the AI Director's dedicated voice settings
    /// (if configured); otherwise `None` inherits the primary broadcast narrator voice.
    pub fn director_voice_override(&self) -> Option<VoiceOverride> {
        let has_wav = !self.director_ref_audio.trim().is_empty();
        let has_preset = !self.director_voice_preset.trim().is_empty();
        if !has_wav && !has_preset {
            return None;
        }
        Some(VoiceOverride {
            reverb_voice_preset: has_preset.then(|| self.director_voice_preset.trim().to_owned()),
            ref_audio: Some(if has_wav {
                self.director_ref_audio.trim().to_owned()
            } else {
                String::new()
            }),
            ref_text: Some(if has_wav {
                self.director_ref_text.trim().to_owned()
            } else {
                String::new()
            }),
            reverb_language: Some(self.director_language.trim().to_owned()),
            tts_voice: None,
            tts_instruction: None,
            seed: None,
        })
    }

    /// Populate BreezeTTS voice settings from a `voice.json` / `voice-design.json` file.
    pub fn load_breeze_voice_json(&mut self, path: &Path) -> Result<(), String> {
        #[derive(Deserialize)]
        struct BreezeVoiceJson {
            #[serde(default)]
            instruction: String,
            #[serde(default = "default_seed")]
            seed: i64,
            #[serde(default = "default_cfg")]
            cfg_scale: f32,
            #[serde(default)]
            reference_audio: String,
            #[serde(default)]
            reference_text: String,
        }
        fn default_seed() -> i64 {
            42
        }
        fn default_cfg() -> f32 {
            1.0
        }
        let raw = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let parsed: BreezeVoiceJson =
            serde_json::from_str(&raw).map_err(|e| format!("Invalid voice.json: {e}"))?;
        self.tts_format = TtsRequestFormat::BreezeMultipart;
        self.tts_instruction = parsed.instruction.trim().to_owned();
        self.tts_ref_audio = parsed.reference_audio.trim().to_owned();
        self.tts_ref_text = parsed.reference_text.trim().to_owned();
        self.tts_cfg_scale = parsed.cfg_scale.clamp(0.1, 10.0);
        self.tts_seed = parsed.seed;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpeechPriority {
    Normal,
    High,
}

/// Optional per-utterance voice override so local agents or the AI Director can
/// speak with their own cloned WAV or preset voice independently of the default narrator.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VoiceOverride {
    pub reverb_voice_preset: Option<String>,
    pub ref_audio: Option<String>,
    pub ref_text: Option<String>,
    pub reverb_language: Option<String>,
    pub tts_voice: Option<String>,
    pub tts_instruction: Option<String>,
    pub seed: Option<i64>,
}

#[derive(Clone, Debug)]
struct SpeechJob {
    text: String,
    source: String,
    priority: SpeechPriority,
    reverb_model_dir: String,
    reverb_voice_preset: String,
    reverb_language: String,
    reverb_device: ReverbDeviceChoice,
    reverb_prefer_int4: bool,
    endpoint: String,
    voice: String,
    model: String,
    format: TtsRequestFormat,
    instruction: String,
    ref_audio: String,
    ref_text: String,
    cfg_scale: f32,
    seed: i64,
    speak_audio: bool,
}

struct QueueInner {
    jobs: VecDeque<SpeechJob>,
    speaking: bool,
    active_caption: Option<(String, String, Instant)>,
    status: String,
    last_error: Option<String>,
    shutdown: bool,
}

pub struct Narrator {
    shared: Arc<(Mutex<QueueInner>, Condvar)>,
    last_narrated_event_id: Option<String>,
    last_auto_at: Option<Instant>,
}

impl Default for Narrator {
    fn default() -> Self {
        let shared = Arc::new((
            Mutex::new(QueueInner {
                jobs: VecDeque::new(),
                speaking: false,
                active_caption: None,
                status: "Idle".into(),
                last_error: None,
                shutdown: false,
            }),
            Condvar::new(),
        ));
        let worker_shared = Arc::clone(&shared);
        let _ = std::thread::Builder::new()
            .name("1kee-narrator".into())
            .spawn(move || run_worker(worker_shared));
        Self {
            shared,
            last_narrated_event_id: None,
            last_auto_at: None,
        }
    }
}

impl Drop for Narrator {
    fn drop(&mut self) {
        let (lock, cvar) = &*self.shared;
        if let Ok(mut guard) = lock.lock() {
            guard.shutdown = true;
            guard.jobs.clear();
            cvar.notify_all();
        }
    }
}

impl Narrator {
    pub fn enqueue(
        &mut self,
        text: String,
        source: impl Into<String>,
        priority: SpeechPriority,
        settings: &AgentVoiceSettings,
    ) {
        self.enqueue_with_voice(text, source, priority, settings, None);
    }

    pub fn enqueue_with_voice(
        &mut self,
        text: String,
        source: impl Into<String>,
        priority: SpeechPriority,
        settings: &AgentVoiceSettings,
        voice_override: Option<VoiceOverride>,
    ) {
        let cleaned = sanitize_for_speech(&text);
        if cleaned.is_empty() {
            return;
        }
        let speak_audio = settings.tts_enabled
            && match settings.tts_format {
                TtsRequestFormat::EmbeddedReverb => !settings.reverb_model_dir.trim().is_empty(),
                _ => !settings.tts_endpoint.trim().is_empty(),
            };
        let ov = voice_override.unwrap_or_default();
        let reverb_voice_preset = ov
            .reverb_voice_preset
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| settings.reverb_voice_preset.trim().to_owned());
        let reverb_language = ov
            .reverb_language
            .filter(|s| reverb_tts::text::SUPPORTED_LANGUAGES.contains(&s.trim()))
            .unwrap_or_else(|| settings.reverb_language.trim().to_owned());
        let ref_audio = ov
            .ref_audio
            .unwrap_or_else(|| settings.tts_ref_audio.trim().to_owned());
        let ref_text = ov
            .ref_text
            .unwrap_or_else(|| settings.tts_ref_text.trim().to_owned());
        let voice = ov
            .tts_voice
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| settings.tts_voice.trim().to_owned());
        let instruction = ov
            .tts_instruction
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| settings.tts_instruction.trim().to_owned());
        let job = SpeechJob {
            text: cleaned,
            source: source.into(),
            priority,
            reverb_model_dir: settings.reverb_model_dir.trim().to_owned(),
            reverb_voice_preset,
            reverb_language,
            reverb_device: settings.reverb_device,
            reverb_prefer_int4: settings.reverb_prefer_int4,
            endpoint: settings.tts_endpoint.trim().to_owned(),
            voice,
            model: settings.tts_model.trim().to_owned(),
            format: settings.tts_format,
            instruction,
            ref_audio,
            ref_text,
            cfg_scale: settings.tts_cfg_scale,
            seed: ov.seed.unwrap_or(settings.tts_seed),
            speak_audio,
        };
        let (lock, cvar) = &*self.shared;
        if let Ok(mut guard) = lock.lock() {
            if job.priority == SpeechPriority::High {
                guard.jobs.push_front(job);
            } else {
                guard.jobs.push_back(job);
            }
            while guard.jobs.len() > 6 {
                guard.jobs.pop_back();
            }
            cvar.notify_one();
        }
        crate::app::request_repaint();
    }

    /// True while a speech job is queued, synthesizing, or actively playing.
    pub fn is_busy(&self) -> bool {
        let (lock, _) = &*self.shared;
        lock.lock()
            .map(|g| g.speaking || !g.jobs.is_empty())
            .unwrap_or(false)
    }

    pub fn tick(&mut self, model: &AppModel, radio_playing: bool) {
        let cfg = &model.agent_voice;
        if !cfg.auto_narrate_follow || !model.event_follow.enabled() || model.follow_recaps.active.is_some() {
            return;
        }
        if cfg.yield_to_radio && radio_playing {
            return;
        }
        let Some(target) = model.event_follow.target() else {
            return;
        };
        if self.last_narrated_event_id.as_deref() == Some(target.id.as_str()) {
            return;
        }
        let cooldown = Duration::from_secs(u64::from(cfg.cooldown_secs.max(5)));
        if self
            .last_auto_at
            .is_some_and(|t| t.elapsed() < cooldown)
        {
            return;
        }
        self.last_narrated_event_id = Some(target.id.clone());
        self.last_auto_at = Some(Instant::now());

        let badge = model.convergence.badge_for_event(&target.id);
        let script = compose_event_narration(target, badge.as_deref());
        let prio = if badge
            .as_deref()
            .is_some_and(|b| b.starts_with("CONVERGENCE"))
        {
            SpeechPriority::High
        } else {
            SpeechPriority::Normal
        };
        self.enqueue(script, "DIRECTOR", prio, cfg);
    }

    /// Returns `(source_tag, caption_text)` if a narration is speaking or recently finished.
    pub fn active_caption(&self) -> Option<(String, String)> {
        let (lock, _) = &*self.shared;
        let guard = lock.lock().ok()?;
        let (src, text, expires) = guard.active_caption.as_ref()?;
        (Instant::now() < *expires).then(|| (src.clone(), text.clone()))
    }

    pub fn status_summary(&self) -> String {
        let (lock, _) = &*self.shared;
        let Ok(guard) = lock.lock() else {
            return "Unavailable".into();
        };
        if let Some(err) = &guard.last_error {
            format!("Error: {err}")
        } else {
            guard.status.clone()
        }
    }

    pub fn last_error(&self) -> Option<String> {
        let (lock, _) = &*self.shared;
        lock.lock().ok()?.last_error.clone()
    }

    pub fn draw_caption_overlay(&self, painter: &egui::Painter, rect: egui::Rect) {
        let Some((source, text)) = self.active_caption() else {
            return;
        };
        let max_w = (rect.width() - 64.0).clamp(220.0, 680.0);
        let label = format!("{source} · {text}");
        let galley = painter.layout(
            label,
            egui::FontId::proportional(13.0),
            egui::Color32::from_rgb(236, 248, 248),
            max_w,
        );
        let pad = egui::vec2(14.0, 8.0);
        let size = galley.size() + pad * 2.0;
        let min = egui::pos2(rect.center().x - size.x * 0.5, rect.bottom() - size.y - 78.0);
        let pill = egui::Rect::from_min_size(min, size);
        painter.rect(
            pill,
            10.0,
            theme::panel_fill(236),
            egui::Stroke::new(1.2, theme::topo_color().gamma_multiply(0.85)),
            egui::StrokeKind::Inside,
        );
        painter.galley(min + pad, galley, egui::Color32::WHITE);
    }
}

/// Strip raw WKT/SRID coordinate strings (e.g. `SRID=4326;POINT (-122.4 37.7)`)
/// and any mention of "Factal" so spoken narrations sound natural.
pub fn sanitize_for_speech(input: &str) -> String {
    let mut text = input.trim().to_owned();
    // Remove any `SRID=<digits>;POINT (...)` or `SRID=<digits>;...(...)` spans.
    while let Some(idx) = text.to_ascii_uppercase().find("SRID=") {
        let rest = &text[idx..];
        let end = if let Some(paren_close) = rest.find(')') {
            idx + paren_close + 1
        } else {
            idx + rest
                .find(|c: char| c.is_whitespace())
                .unwrap_or(rest.len())
        };
        text.replace_range(idx..end, " ");
    }
    // Remove case-insensitive mentions of "Factal" (including trailing colon/possessive).
    loop {
        let lower = text.to_ascii_lowercase();
        let Some(idx) = lower.find("factal") else {
            break;
        };
        let mut end = idx + "factal".len();
        let bytes = text.as_bytes();
        if bytes.get(end..end + 2) == Some(b"'s") || bytes.get(end..end + 2) == Some(b"'S") {
            end += 2;
        }
        while end < bytes.len() && matches!(bytes[end], b':' | b'-' | b',' | b';') {
            end += 1;
        }
        // Also strip a preceding "via " / "per " / "from " / "according to " when it only led into Factal.
        let mut start = idx;
        for prefix in ["according to ", "reported by ", "via ", "per ", "from "] {
            if lower[..start].ends_with(prefix) {
                start -= prefix.len();
                break;
            }
        }
        text.replace_range(start..end, " ");
    }
    // Collapse whitespace and clean stray punctuation spacing.
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed
        .replace(" .", ".")
        .replace(" ,", ",")
        .replace(" ;", ";")
        .replace(" :", ":")
        .trim_matches(|c: char| c.is_whitespace() || matches!(c, '-' | ':' | ';' | ','))
        .to_owned()
}

pub fn compose_event_narration(event: &crate::model::EventRecord, _badge: Option<&str>) -> String {
    let raw = if !event.summary.trim().is_empty() {
        event.summary.as_str()
    } else {
        event.title.as_str()
    };
    let mut out = sanitize_for_speech(raw);
    if out.is_empty() {
        out = sanitize_for_speech(&event.title);
    }
    if !out.is_empty() && !out.ends_with(['.', '!', '?']) {
        out.push('.');
    }
    out
}

pub fn build_tts_payload(
    text: &str,
    voice: &str,
    model: &str,
    format: TtsRequestFormat,
) -> serde_json::Value {
    match format {
        TtsRequestFormat::OpenAiSpeech
        | TtsRequestFormat::BreezeMultipart
        | TtsRequestFormat::EmbeddedReverb => serde_json::json!({
            "model": if model.is_empty() { "breeze" } else { model },
            "input": text,
            "voice": if voice.is_empty() { "default" } else { voice },
            "response_format": "wav"
        }),
        TtsRequestFormat::SimpleText => serde_json::json!({
            "text": text,
            "voice": if voice.is_empty() { "default" } else { voice }
        }),
    }
}

/// Build a `multipart/form-data` request body compatible with BreezeTTS's
/// FastAPI `/v1/audio/speech` endpoint (`text`, `cfg_scale`, `seed`, optional
/// `instruction`, and optional paired `ref_audio` + `ref_text`).
pub fn build_breeze_multipart(
    text: &str,
    instruction: &str,
    ref_audio_bytes: Option<&[u8]>,
    ref_text: &str,
    cfg_scale: f32,
    seed: i64,
    boundary: &str,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() + ref_audio_bytes.map_or(256, |b| b.len() + 512));
    let has_ref = ref_audio_bytes.is_some_and(|b| !b.is_empty()) && !ref_text.trim().is_empty();
    let effective_instruction = if has_ref {
        instruction.trim()
    } else if instruction.trim().is_empty() {
        DEFAULT_BREEZE_INSTRUCTION
    } else {
        instruction.trim()
    };
    let effective_cfg = if has_ref {
        cfg_scale.clamp(0.1, 10.0)
    } else if cfg_scale <= 1.05 {
        4.0
    } else {
        cfg_scale.clamp(0.1, 10.0)
    };

    let mut push_field = |name: &str, value: &str| {
        out.extend_from_slice(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n")
                .as_bytes(),
        );
    };
    push_field("text", text);
    push_field("cfg_scale", &format!("{effective_cfg:.2}"));
    push_field("seed", &seed.to_string());
    if !effective_instruction.is_empty() {
        push_field("instruction", effective_instruction);
    }
    if has_ref {
        push_field("ref_text", ref_text.trim());
        out.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"ref_audio\"; filename=\"reference.wav\"\r\nContent-Type: audio/wav\r\n\r\n"
            )
            .as_bytes(),
        );
        out.extend_from_slice(ref_audio_bytes.unwrap());
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    out
}

/// Wrap raw mono 16-bit signed little-endian PCM bytes in a standard 44-byte RIFF/WAVE header.
pub fn wrap_pcm16_as_wav(pcm: &[u8], sample_rate: u32) -> Vec<u8> {
    let data_len = (pcm.len() & !1) as u32;
    let byte_rate = sample_rate.saturating_mul(2);
    let mut wav = Vec::with_capacity(44 + data_len as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36u32.saturating_add(data_len)).to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes()); // PCM fmt chunk size
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM format = 1
    wav.extend_from_slice(&1u16.to_le_bytes()); // Mono
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&byte_rate.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes()); // Block align = 2 bytes
    wav.extend_from_slice(&16u16.to_le_bytes()); // Bits per sample = 16
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    wav.extend_from_slice(&pcm[..data_len as usize]);
    wav
}

fn send_breeze_request(client: &Client, job: &SpeechJob) -> Result<(Vec<u8>, bool, u32), String> {
    let ref_bytes = if !job.ref_audio.is_empty() && !job.ref_text.is_empty() {
        match std::fs::read(&job.ref_audio) {
            Ok(b) if !b.is_empty() => Some(b),
            Ok(_) => return Err(format!("Reference audio file is empty: {}", job.ref_audio)),
            Err(e) => return Err(format!("Cannot read reference audio {}: {e}", job.ref_audio)),
        }
    } else {
        None
    };
    let boundary = format!(
        "1kee-breeze-{:x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    );
    let body = build_breeze_multipart(
        &job.text,
        &job.instruction,
        ref_bytes.as_deref(),
        &job.ref_text,
        job.cfg_scale,
        job.seed,
        &boundary,
    );
    let content_type = format!("multipart/form-data; boundary={boundary}");
    for attempt in 0..12 {
        let resp = client
            .post(&job.endpoint)
            .header(reqwest::header::CONTENT_TYPE, &content_type)
            .body(body.clone())
            .send()
            .map_err(|e| format!("TTS request failed: {e}"))?;
        if resp.status().as_u16() == 409 && attempt < 11 {
            std::thread::sleep(Duration::from_secs(2));
            continue;
        }
        if !resp.status().is_success() {
            let status = resp.status();
            let detail = resp.text().unwrap_or_default();
            let detail = detail.trim();
            return Err(if detail.is_empty() {
                format!("TTS HTTP {status}")
            } else {
                format!("TTS HTTP {status}: {}", &detail[..detail.len().min(160)])
            });
        }
        let is_pcm = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|ct| ct.contains("audio/pcm") || ct.contains("octet-stream"))
            || resp
                .headers()
                .get("x-sample-format")
                .and_then(|v| v.to_str().ok())
                .is_some_and(|f| f.eq_ignore_ascii_case("s16le"));
        let sample_rate = resp
            .headers()
            .get("x-sample-rate")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(24_000);
        let bytes = resp
            .bytes()
            .map_err(|e| format!("TTS stream read failed: {e}"))?;
        if bytes.is_empty() {
            return Err("TTS returned empty audio body".into());
        }
        return Ok((bytes.to_vec(), is_pcm, sample_rate));
    }
    Err("BreezeTTS remained busy (HTTP 409)".into())
}

fn synthesize_audio(client: &Client, job: &SpeechJob) -> Result<Vec<u8>, String> {
    if job.format == TtsRequestFormat::BreezeMultipart {
        let (bytes, is_pcm, sample_rate) = send_breeze_request(client, job)?;
        let already_framed = bytes.len() >= 4
            && (&bytes[..4] == b"RIFF" || &bytes[..4] == b"OggS" || &bytes[..3] == b"ID3");
        return Ok(if is_pcm || !already_framed {
            wrap_pcm16_as_wav(&bytes, sample_rate)
        } else {
            bytes
        });
    }

    let payload = build_tts_payload(&job.text, &job.voice, &job.model, job.format);
    let resp = client
        .post(&job.endpoint)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(payload.to_string())
        .send()
        .map_err(|e| format!("TTS request failed: {e}"))?;

    // If a FastAPI BreezeTTS server rejects JSON with 422, seamlessly retry as multipart PCM.
    if resp.status().as_u16() == 422 && job.format == TtsRequestFormat::OpenAiSpeech {
        let (bytes, is_pcm, sample_rate) = send_breeze_request(client, job)?;
        let already_framed = bytes.len() >= 4
            && (&bytes[..4] == b"RIFF" || &bytes[..4] == b"OggS" || &bytes[..3] == b"ID3");
        return Ok(if is_pcm || !already_framed {
            wrap_pcm16_as_wav(&bytes, sample_rate)
        } else {
            bytes
        });
    }

    if !resp.status().is_success() {
        return Err(format!("TTS HTTP {}", resp.status()));
    }
    let is_pcm = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.contains("audio/pcm"));
    let sample_rate = resp
        .headers()
        .get("x-sample-rate")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(24_000);
    let bytes = resp
        .bytes()
        .map_err(|e| format!("TTS read failed: {e}"))?;
    if bytes.is_empty() {
        return Err("TTS returned empty audio body".into());
    }
    Ok(if is_pcm {
        wrap_pcm16_as_wav(&bytes, sample_rate)
    } else {
        bytes.to_vec()
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ReverbEngineKey {
    model_dir: String,
    requested_device: ReverbDeviceChoice,
    prefer_int4: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ReverbVoiceKey {
    Preset(String),
    ClonedWav {
        path: String,
        transcript: String,
        language: String,
        file_len: u64,
    },
}

struct CachedReverb {
    key: ReverbEngineKey,
    active_device: ReverbDeviceChoice,
    engine: reverb_tts::ReverbEngine,
    voice_key: ReverbVoiceKey,
    voice: reverb_tts::Voice,
}

fn resolve_reverb_voice_key(job: &SpeechJob) -> Result<ReverbVoiceKey, String> {
    if !job.ref_audio.is_empty() {
        if job.ref_text.is_empty() {
            return Err("Reference WAV requires its exact transcript for voice cloning.".into());
        }
        let meta = std::fs::metadata(&job.ref_audio)
            .map_err(|e| format!("Cannot stat reference WAV {}: {e}", job.ref_audio))?;
        let lang = if job.reverb_language.is_empty() {
            "en".to_owned()
        } else {
            job.reverb_language.clone()
        };
        Ok(ReverbVoiceKey::ClonedWav {
            path: job.ref_audio.clone(),
            transcript: job.ref_text.clone(),
            language: lang,
            file_len: meta.len(),
        })
    } else {
        let voice_id = if job.reverb_voice_preset.is_empty() {
            "en_male_narrator_1".to_owned()
        } else {
            job.reverb_voice_preset.clone()
        };
        Ok(ReverbVoiceKey::Preset(voice_id))
    }
}

fn load_or_clone_voice(
    engine: &reverb_tts::ReverbEngine,
    voice_key: &ReverbVoiceKey,
    lock: &Mutex<QueueInner>,
) -> Result<reverb_tts::Voice, String> {
    match voice_key {
        ReverbVoiceKey::Preset(voice_id) => {
            let p = Path::new(voice_id);
            if p.is_file() {
                reverb_tts::Voice::load_json(p)
            } else {
                engine.load_preset_voice(voice_id)
            }
        }
        ReverbVoiceKey::ClonedWav {
            path,
            transcript,
            language,
            ..
        } => {
            set_status(lock, "Cloning voice from reference WAV…");
            engine.clone_voice_from_wav(Path::new(path), transcript, language)
        }
    }
}

fn device_fallback_order(requested: ReverbDeviceChoice) -> Vec<ReverbDeviceChoice> {
    match requested {
        ReverbDeviceChoice::Cuda0 => vec![
            ReverbDeviceChoice::Cuda0,
            ReverbDeviceChoice::Cuda1,
            ReverbDeviceChoice::Cpu,
        ],
        ReverbDeviceChoice::Cuda1 => vec![
            ReverbDeviceChoice::Cuda1,
            ReverbDeviceChoice::Cuda0,
            ReverbDeviceChoice::Cpu,
        ],
        ReverbDeviceChoice::Cpu => vec![ReverbDeviceChoice::Cpu],
    }
}

fn try_reverb_on_device(
    cached: &mut Option<CachedReverb>,
    want_key: &ReverbEngineKey,
    want_voice: &ReverbVoiceKey,
    device: ReverbDeviceChoice,
    job: &SpeechJob,
    lock: &Mutex<QueueInner>,
) -> Result<(Vec<u8>, String), String> {
    let needs_reload = cached
        .as_ref()
        .is_none_or(|c| c.key != *want_key || c.active_device != device);
    if needs_reload {
        *cached = None;
        let fallback_note = if device != want_key.requested_device {
            " (auto-fallback)"
        } else {
            ""
        };
        set_status(
            lock,
            &format!(
                "Loading ReverbTTS ({}) on {}{fallback_note}…",
                if want_key.prefer_int4 { "INT4" } else { "FP16" },
                device.label()
            ),
        );
        let engine = reverb_tts::ReverbEngine::load(
            Path::new(&want_key.model_dir),
            want_key.prefer_int4,
            device.into_compute(),
        )?;
        let voice = load_or_clone_voice(&engine, want_voice, lock)?;
        *cached = Some(CachedReverb {
            key: want_key.clone(),
            active_device: device,
            engine,
            voice_key: want_voice.clone(),
            voice,
        });
    }

    let state = cached.as_mut().unwrap();
    if state.voice_key != *want_voice {
        state.voice = load_or_clone_voice(&state.engine, want_voice, lock)?;
        state.voice_key = want_voice.clone();
    }

    set_status(lock, "Synthesizing (ReverbTTS)…");
    let t0 = Instant::now();
    let wav = state
        .engine
        .generate_wav(&job.text, &state.voice, job.seed.max(0) as u64)?;
    let dt = t0.elapsed().as_secs_f32();
    let voice_tag = match &state.voice_key {
        ReverbVoiceKey::ClonedWav { .. } => format!("cloned '{}'", state.voice.name),
        ReverbVoiceKey::Preset(_) => state.voice.name.clone(),
    };
    let fb = if state.active_device != want_key.requested_device {
        " · auto-fallback"
    } else {
        ""
    };
    let info = format!(
        "Ready · Reverb {} ({voice_tag}) / {}{fb} ({dt:.2}s)",
        state.engine.variant_label(),
        state.engine.device_label()
    );
    Ok((wav, info))
}

fn synthesize_embedded_reverb(
    cached: &mut Option<CachedReverb>,
    job: &SpeechJob,
    lock: &Mutex<QueueInner>,
) -> Result<(Vec<u8>, String), String> {
    let want_key = ReverbEngineKey {
        model_dir: job.reverb_model_dir.clone(),
        requested_device: job.reverb_device,
        prefer_int4: job.reverb_prefer_int4,
    };
    let want_voice = resolve_reverb_voice_key(job)?;

    // Fast path: if we already have a healthy engine cached for this user request
    // (including a previously auto-fallen-back GPU), try it first!
    if let Some(existing) = cached.as_ref()
        && existing.key == want_key
    {
        let active = existing.active_device;
        match try_reverb_on_device(cached, &want_key, &want_voice, active, job, lock) {
            Ok(ok) => return Ok(ok),
            Err(_) => {
                *cached = None;
            }
        }
    }

    let candidates = device_fallback_order(want_key.requested_device);
    let mut last_err = String::new();
    for (idx, device) in candidates.iter().copied().enumerate() {
        match try_reverb_on_device(cached, &want_key, &want_voice, device, job, lock) {
            Ok(ok) => return Ok(ok),
            Err(err) => {
                *cached = None;
                last_err = err.clone();
                if idx + 1 < candidates.len() {
                    let next = candidates[idx + 1];
                    set_status(
                        lock,
                        &format!(
                            "{} busy/OOM — switching to {}…",
                            device.label(),
                            next.label()
                        ),
                    );
                }
            }
        }
    }
    Err(last_err)
}

fn run_worker(shared: Arc<(Mutex<QueueInner>, Condvar)>) {
    let client = Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .ok();
    let (lock, cvar) = &*shared;
    let mut cached_reverb: Option<CachedReverb> = None;

    loop {
        let job = {
            let mut guard = match lock.lock() {
                Ok(g) => g,
                Err(_) => return,
            };
            while guard.jobs.is_empty() && !guard.shutdown {
                guard = match cvar.wait(guard) {
                    Ok(g) => g,
                    Err(_) => return,
                };
            }
            if guard.shutdown {
                return;
            }
            guard.speaking = true;
            guard.jobs.pop_front().unwrap()
        };

        // Show caption while synthesizing & speaking.
        if let Ok(mut guard) = lock.lock() {
            guard.active_caption = Some((
                job.source.clone(),
                job.text.clone(),
                Instant::now() + Duration::from_secs(60),
            ));
            guard.status = if job.speak_audio {
                "Synthesizing audio…".into()
            } else {
                "Caption only (TTS disabled or model folder not set)".into()
            };
            guard.last_error = None;
        }
        crate::app::request_repaint();

        if job.speak_audio {
            if job.format == TtsRequestFormat::EmbeddedReverb {
                match synthesize_embedded_reverb(&mut cached_reverb, &job, lock) {
                    Ok((audio_bytes, ready_status)) => {
                        set_status(lock, "Playing audio…");
                        if let Err(err) = play_audio_bytes(&audio_bytes) {
                            set_error(lock, err);
                        } else {
                            set_status(lock, &ready_status);
                        }
                    }
                    Err(err) => {
                        set_error(lock, err);
                    }
                }
            } else if let Some(ref client) = client {
                // Free any cached in-process GPU model when switching to an external HTTP backend.
                cached_reverb = None;
                match synthesize_audio(client, &job) {
                    Ok(audio_bytes) => {
                        set_status(lock, "Playing audio…");
                        if let Err(err) = play_audio_bytes(&audio_bytes) {
                            set_error(lock, err);
                        } else {
                            set_status(lock, "Connected · Ready");
                        }
                    }
                    Err(err) => {
                        set_error(lock, err);
                    }
                }
            }
        } else {
            // Caption-only mode: hold caption cleanly for 5 seconds before popping next item.
            std::thread::sleep(Duration::from_secs(5));
            set_status(lock, "Idle");
        }

        if let Ok(mut guard) = lock.lock() {
            guard.speaking = false;
            guard.active_caption = Some((
                job.source,
                job.text,
                Instant::now() + Duration::from_secs(4),
            ));
        }
        crate::app::request_repaint();
    }
}

fn set_status(lock: &Mutex<QueueInner>, msg: &str) {
    if let Ok(mut guard) = lock.lock() {
        guard.status = msg.to_owned();
    }
    crate::app::request_repaint();
}

fn set_error(lock: &Mutex<QueueInner>, msg: String) {
    if let Ok(mut guard) = lock.lock() {
        guard.last_error = Some(msg);
    }
    crate::app::request_repaint();
}

fn play_audio_bytes(bytes: &[u8]) -> Result<(), String> {
    let ext = if bytes.len() >= 4 && &bytes[..4] == b"RIFF" {
        "wav"
    } else if bytes.len() >= 4 && &bytes[..4] == b"OggS" {
        "ogg"
    } else {
        "mp3"
    };
    let tmp = tempfile::Builder::new()
        .prefix("1kee_tts_")
        .suffix(&format!(".{ext}"))
        .tempfile()
        .map_err(|e| format!("Audio temp file failed: {e}"))?;
    std::fs::write(tmp.path(), bytes).map_err(|e| format!("Audio write failed: {e}"))?;
    let path = tmp.path();
    #[cfg(target_os = "macos")]
    {
        let ok = Command::new("afplay")
            .arg(path)
            .status()
            .is_ok_and(|s| s.success());
        return if ok { Ok(()) } else { Err("afplay failed".into()) };
    }
    #[cfg(not(target_os = "macos"))]
    {
        // Try PipeWire / PulseAudio / FFmpeg / ALSA playback tools in order.
        let played = Command::new("pw-play")
            .arg(path)
            .status()
            .is_ok_and(|s| s.success())
            || Command::new("paplay")
                .arg(path)
                .status()
                .is_ok_and(|s| s.success())
            || Command::new("ffplay")
                .args(["-nodisp", "-autoexit", "-loglevel", "quiet"])
                .arg(path)
                .status()
                .is_ok_and(|s| s.success())
            || (ext == "wav"
                && Command::new("aplay")
                    .arg("-q")
                    .arg(path)
                    .status()
                    .is_ok_and(|s| s.success()));
        if played {
            Ok(())
        } else {
            Err("Local audio playback failed (pw-play/paplay/ffplay/aplay)".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{EventRecord, EventSeverity, GeoPoint};

    #[test]
    fn builds_openai_and_simple_tts_payloads() {
        let oa = build_tts_payload("Alert in Tokyo", "aria", "breeze", TtsRequestFormat::OpenAiSpeech);
        assert_eq!(oa["input"], "Alert in Tokyo");
        assert_eq!(oa["voice"], "aria");
        assert_eq!(oa["response_format"], "wav");

        let simple = build_tts_payload("Hello", "neon", "breeze", TtsRequestFormat::SimpleText);
        assert_eq!(simple["text"], "Hello");
        assert_eq!(simple["voice"], "neon");
    }

    #[test]
    fn composes_clean_summary_narration_without_srid_or_factal() {
        let ev = EventRecord {
            id: "factal-1".into(),
            title: "Factal: Wildfire evacuation ordered".into(),
            summary: "Wildfire evacuation ordered near Napa Valley via Factal at SRID=4326;POINT (-122.3 38.3)".into(),
            severity: EventSeverity::Critical,
            location_name: "SRID=4326;POINT (-122.3 38.3)".into(),
            location: GeoPoint { lat: 38.3, lon: -122.3 },
            source: "Factal".into(),
            occurred_at: String::new(),
            factal_brief: None,
        };
        let text = compose_event_narration(&ev, Some("CONVERGENCE · FACTAL + 12 FIRES + 2 CAMS"));
        assert!(text.contains("Wildfire evacuation ordered near Napa Valley"), "{text}");
        assert!(!text.to_ascii_lowercase().contains("factal"), "{text}");
        assert!(!text.to_ascii_uppercase().contains("SRID"), "{text}");
        assert!(!text.contains("POINT"), "{text}");
        assert!(text.ends_with('.'), "{text}");
    }

    #[test]
    fn builds_breeze_multipart_and_wraps_pcm_as_wav() {
        let body = build_breeze_multipart(
            "Hello world",
            "",
            Some(b"RIFF\x00\x00\x00\x00WAVE"),
            "Reference line",
            1.0,
            42,
            "test-bound",
        );
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("name=\"text\"\r\n\r\nHello world"));
        assert!(text.contains("name=\"cfg_scale\"\r\n\r\n1.00"));
        assert!(text.contains("name=\"ref_text\"\r\n\r\nReference line"));
        assert!(text.contains("name=\"ref_audio\"; filename=\"reference.wav\""));

        let pcm = vec![0u8; 4800];
        let wav = wrap_pcm16_as_wav(&pcm, 24_000);
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 24_000);
        assert_eq!(wav.len(), 44 + 4800);
    }
}
