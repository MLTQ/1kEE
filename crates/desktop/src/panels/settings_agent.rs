//! Opt-in Broadcast Narrator (Built-in Rust ReverbTTS + HTTP TTS) & Loopback Local Agent Harness settings.
use crate::model::AppModel;
use crate::narrator::{ReverbDeviceChoice, TtsRequestFormat};
use crate::theme;
use std::path::Path;

pub(super) fn render(ui: &mut egui::Ui, model: &mut AppModel) {
    ui.heading("Broadcast Narrator");
    ui.colored_label(
        theme::text_muted(),
        "Speak live Follow Events summaries and local agent commentary using the built-in pure-Rust \
         ReverbTTS runner (zero Python / zero sidecars) or an external local HTTP TTS server.",
    );
    ui.add_space(6.0);

    ui.checkbox(
        &mut model.agent_voice.tts_enabled,
        "Enable audio narration",
    );
    ui.checkbox(
        &mut model.agent_voice.auto_narrate_follow,
        "Auto-narrate Follow Events stops",
    );
    ui.checkbox(
        &mut model.agent_voice.yield_to_radio,
        "Yield narration when Nearby Radio is playing",
    );
    ui.add_space(6.0);

    ui.horizontal_wrapped(|ui| {
        ui.label("TTS Backend");
        for fmt in [
            TtsRequestFormat::EmbeddedReverb,
            TtsRequestFormat::BreezeMultipart,
            TtsRequestFormat::OpenAiSpeech,
            TtsRequestFormat::SimpleText,
        ] {
            ui.selectable_value(&mut model.agent_voice.tts_format, fmt, fmt.label());
        }
    });
    ui.add_space(6.0);

    if model.agent_voice.tts_format == TtsRequestFormat::EmbeddedReverb {
        ui.label("ReverbTTS Checkpoints Folder (contains Reverb-TTS/ & higgs-audio-v2-tokenizer/)");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Browse…").clicked()
                && let Some(dir) = rfd::FileDialog::new().pick_folder()
            {
                model.agent_voice.reverb_model_dir = dir.display().to_string();
            }
            ui.add_sized(
                [ui.available_width().max(120.0), 24.0],
                egui::TextEdit::singleline(&mut model.agent_voice.reverb_model_dir)
                    .hint_text("/run/media/m/Minkowski/Models/ReverbTTS"),
            );
        });
        ui.add_space(4.0);

        ui.label("Reference WAV (optional — zero-shot voice cloning, 3–15s clean speech)");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if !model.agent_voice.tts_ref_audio.trim().is_empty()
                && ui.small_button("Clear").clicked()
            {
                model.agent_voice.tts_ref_audio.clear();
            }
            if ui.button("Browse WAV…").clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .add_filter("WAV audio", &["wav"])
                    .pick_file()
            {
                model.agent_voice.tts_ref_audio = path.display().to_string();
            }
            ui.add_sized(
                [ui.available_width().max(120.0), 24.0],
                egui::TextEdit::singleline(&mut model.agent_voice.tts_ref_audio)
                    .hint_text("Leave blank to use a preset voice below"),
            );
        });

        if !model.agent_voice.tts_ref_audio.trim().is_empty() {
            ui.label("Reference WAV exact transcript (required for voice cloning)");
            ui.add_sized(
                [ui.available_width(), 28.0],
                egui::TextEdit::singleline(&mut model.agent_voice.tts_ref_text)
                    .hint_text("Type the exact words spoken in the reference WAV"),
            );
            ui.horizontal_wrapped(|ui| {
                ui.label("Language");
                egui::ComboBox::from_id_salt("reverb_language")
                    .width(80.0)
                    .selected_text(&model.agent_voice.reverb_language)
                    .show_ui(ui, |ui| {
                        for &lang in reverb_tts::text::SUPPORTED_LANGUAGES {
                            ui.selectable_value(
                                &mut model.agent_voice.reverb_language,
                                lang.to_owned(),
                                lang,
                            );
                        }
                    });
                ui.small("Encodes once on demand (~1.5s); encoder frees VRAM immediately after.");
            });
        } else {
            let dir_key = model.agent_voice.reverb_model_dir.trim().to_owned();
            let presets: Vec<String> = if dir_key.is_empty() {
                Vec::new()
            } else {
                let cache_id = egui::Id::new("reverb-preset-voices-cache");
                ui.ctx().data_mut(|d| {
                    if let Some((cached_dir, cached_list)) =
                        d.get_temp::<(String, Vec<String>)>(cache_id)
                        && cached_dir == dir_key
                    {
                        return cached_list;
                    }
                    let list = reverb_tts::list_preset_voices(Path::new(&dir_key));
                    d.insert_temp(cache_id, (dir_key.clone(), list.clone()));
                    list
                })
            };
            ui.horizontal_wrapped(|ui| {
                ui.label("Preset Voice");
                if !presets.is_empty() {
                    egui::ComboBox::from_id_salt("reverb_voice_preset")
                        .width(210.0)
                        .selected_text(&model.agent_voice.reverb_voice_preset)
                        .show_ui(ui, |ui| {
                            for name in &presets {
                                ui.selectable_value(
                                    &mut model.agent_voice.reverb_voice_preset,
                                    name.clone(),
                                    name,
                                );
                            }
                        });
                } else {
                    ui.add(
                        egui::TextEdit::singleline(&mut model.agent_voice.reverb_voice_preset)
                            .desired_width(190.0)
                            .hint_text("en_male_narrator_1"),
                    );
                }
                if ui.small_button("Voice JSON…").clicked()
                    && let Some(path) = rfd::FileDialog::new()
                        .add_filter("Voice JSON", &["json"])
                        .pick_file()
                {
                    model.agent_voice.reverb_voice_preset = path.display().to_string();
                }
            });
        }
        ui.add_space(4.0);

        ui.horizontal_wrapped(|ui| {
            ui.label("Device");
            for dev in [
                ReverbDeviceChoice::Cuda0,
                ReverbDeviceChoice::Cuda1,
                ReverbDeviceChoice::Cpu,
            ] {
                ui.selectable_value(&mut model.agent_voice.reverb_device, dev, dev.label());
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(
                &mut model.agent_voice.reverb_prefer_int4,
                "Prefer INT4 checkpoint (182 MB)",
            );
            ui.label("Seed");
            ui.add(egui::DragValue::new(&mut model.agent_voice.tts_seed));
        });
        ui.small("Loads lazily on first narration (~0.2s per sentence on GPU). CUDA 0 is your fastest GPU; CUDA 1 is your secondary GPU.");
    } else {
        ui.label("TTS Endpoint URL");
        ui.add_sized(
            [ui.available_width(), 28.0],
            egui::TextEdit::singleline(&mut model.agent_voice.tts_endpoint)
                .hint_text("http://127.0.0.1:7866/v1/audio/speech"),
        );
        ui.add_space(4.0);
        if model.agent_voice.tts_format == TtsRequestFormat::BreezeMultipart {
            ui.horizontal(|ui| {
                if ui.button("Load Breeze voice.json…").clicked()
                    && let Some(path) = rfd::FileDialog::new()
                        .add_filter("JSON", &["json"])
                        .pick_file()
                {
                    match model.agent_voice.load_breeze_voice_json(&path) {
                        Ok(()) => {
                            model.push_log(format!(
                                "Loaded Breeze voice config from {}",
                                path.display()
                            ));
                        }
                        Err(err) => {
                            model.push_log(format!("Breeze voice.json error: {err}"));
                        }
                    }
                }
                ui.label("CFG");
                ui.add(
                    egui::DragValue::new(&mut model.agent_voice.tts_cfg_scale)
                        .speed(0.1)
                        .range(0.1..=10.0),
                );
                ui.label("Seed");
                ui.add(egui::DragValue::new(&mut model.agent_voice.tts_seed));
            });
            ui.add_space(4.0);
            ui.label("Reference WAV (optional — fast voice cloning at CFG 1.0)");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Browse…").clicked()
                    && let Some(path) = rfd::FileDialog::new()
                        .add_filter("WAV audio", &["wav"])
                        .pick_file()
                {
                    model.agent_voice.tts_ref_audio = path.display().to_string();
                }
                ui.add_sized(
                    [ui.available_width().max(120.0), 24.0],
                    egui::TextEdit::singleline(&mut model.agent_voice.tts_ref_audio)
                        .hint_text("/path/to/reference.wav"),
                );
            });
            if !model.agent_voice.tts_ref_audio.trim().is_empty() {
                ui.label("Reference WAV exact transcript");
                ui.add_sized(
                    [ui.available_width(), 28.0],
                    egui::TextEdit::singleline(&mut model.agent_voice.tts_ref_text)
                        .hint_text("Exact spoken words of the reference WAV"),
                );
            } else {
                ui.label("Voice style instruction (used when no reference WAV is set, CFG 4.0)");
                ui.add_sized(
                    [ui.available_width(), 28.0],
                    egui::TextEdit::singleline(&mut model.agent_voice.tts_instruction)
                        .hint_text("A calm, clear broadcast narrator with natural pacing."),
                );
            }
        } else {
            ui.horizontal(|ui| {
                ui.label("Voice");
                ui.add(
                    egui::TextEdit::singleline(&mut model.agent_voice.tts_voice)
                        .desired_width(120.0)
                        .hint_text("default"),
                );
                ui.label("Model");
                ui.add(
                    egui::TextEdit::singleline(&mut model.agent_voice.tts_model)
                        .desired_width(120.0)
                        .hint_text("breeze"),
                );
            });
        }
    }
    ui.add_space(4.0);
    ui.add(
        egui::Slider::new(&mut model.agent_voice.cooldown_secs, 5..=300)
            .suffix(" s")
            .text("Auto-narration cooldown"),
    );
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        if ui.button("Test Narrator Caption & Voice").clicked() {
            model.pending_test_narration = Some(
                "One Thousand Electric Eye broadcast narrator online. Ready for live coverage.".into(),
            );
        }
        ui.small(format!("Status: {}", model.narrator_status));
    });

    ui.add_space(12.0);
    ui.separator();
    ui.add_space(8.0);
    ui.heading("Autonomous Local AI Agent Director");
    ui.colored_label(
        theme::text_muted(),
        "Let your local LLM actively pilot 1kEE: analyze live events, terrain, and map screenshots, \
         fly smoothly between Globe and Local 3D views, toggle layers/themes/viewshed, drop map \
         annotations, and speak updates in its own voice.",
    );
    ui.add_space(6.0);
    ui.horizontal_wrapped(|ui| {
        if ui
            .checkbox(
                &mut model.agent_voice.director_enabled,
                "Enable AI Agent autopilot (also toggleable via AI AGENT on top bar)",
            )
            .changed()
            && model.agent_voice.director_enabled
        {
            model.stop_event_follow();
            model.globe_view.stop_motion();
        }
    });
    ui.horizontal_wrapped(|ui| {
        if ui.button("Run One Step Now").clicked() {
            model.pending_director_step = true;
        }
        if ui.button("Detect Model").clicked() {
            model.pending_director_detect = true;
        }
        ui.small(format!("Status: {}", model.agent_director_status));
    });
    if !model.agent_director_last_action.is_empty() {
        ui.small(format!("Last action: {}", model.agent_director_last_action));
    }
    if !model.agent_director_last_thought.is_empty() {
        ui.colored_label(
            theme::text_muted(),
            egui::RichText::new(format!("Reasoning: {}", model.agent_director_last_thought)).small(),
        );
    }
    ui.add_space(6.0);

    ui.label("Local LLM Base URL (OpenAI-compatible /v1)");
    ui.add_sized(
        [ui.available_width(), 24.0],
        egui::TextEdit::singleline(&mut model.agent_voice.director_llm_url)
            .hint_text("http://127.0.0.1:8003/v1"),
    );
    ui.add_space(4.0);

    ui.horizontal_wrapped(|ui| {
        ui.label("Model");
        if !model.agent_director_detected_models.is_empty() {
            egui::ComboBox::from_id_salt("director_model_combo")
                .width(220.0)
                .selected_text(&model.agent_voice.director_model)
                .show_ui(ui, |ui| {
                    for m in &model.agent_director_detected_models {
                        ui.selectable_value(
                            &mut model.agent_voice.director_model,
                            m.clone(),
                            m,
                        );
                    }
                });
        } else {
            ui.add(
                egui::TextEdit::singleline(&mut model.agent_voice.director_model)
                    .desired_width(220.0)
                    .hint_text("inkling-2-mini-abliterated"),
            );
        }
        ui.label("Callsign");
        ui.add(
            egui::TextEdit::singleline(&mut model.agent_voice.director_persona_name)
                .desired_width(100.0)
                .hint_text("ATLAS-1"),
        );
    });
    ui.add_space(4.0);

    ui.horizontal_wrapped(|ui| {
        ui.add(
            egui::Slider::new(&mut model.agent_voice.director_interval_secs, 8..=300)
                .suffix(" s")
                .text("Update interval"),
        );
        ui.checkbox(
            &mut model.agent_voice.director_send_screenshot,
            "Send live map screenshot (Vision)",
        );
    });
    ui.add_space(4.0);

    ui.horizontal_wrapped(|ui| {
        ui.label("Agent Behavior & Personality (System Prompt)");
        if ui.small_button("Reset Default").clicked() {
            model.agent_voice.director_system_prompt =
                crate::narrator::DEFAULT_DIRECTOR_SYSTEM_PROMPT.into();
        }
    });
    ui.add_sized(
        [ui.available_width(), 92.0],
        egui::TextEdit::multiline(&mut model.agent_voice.director_system_prompt)
            .desired_rows(4),
    );
    ui.add_space(4.0);

    ui.label("Direct Instruction / Nudge (steers the agent immediately on its next turn)");
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let send_clicked = ui.button("Send").clicked();
        let edit_resp = ui.add_sized(
            [ui.available_width().max(120.0), 24.0],
            egui::TextEdit::singleline(&mut model.agent_director_nudge_input)
                .hint_text("e.g. Switch to Atlas mode, inspect Europe in 3D, and check live flights"),
        );
        let enter = edit_resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if (send_clicked || enter) && !model.agent_director_nudge_input.trim().is_empty() {
            model.pending_director_nudge = Some(model.agent_director_nudge_input.trim().to_owned());
            model.agent_director_nudge_input.clear();
        }
    });
    ui.add_space(6.0);

    ui.strong("Agent Dedicated Voice (optional — leave blank to use main Narrator voice)");
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if !model.agent_voice.director_ref_audio.trim().is_empty()
            && ui.small_button("Clear").clicked()
        {
            model.agent_voice.director_ref_audio.clear();
        }
        if ui.button("Agent WAV…").clicked()
            && let Some(path) = rfd::FileDialog::new()
                .add_filter("WAV audio", &["wav"])
                .pick_file()
        {
            model.agent_voice.director_ref_audio = path.display().to_string();
        }
        ui.add_sized(
            [ui.available_width().max(120.0), 24.0],
            egui::TextEdit::singleline(&mut model.agent_voice.director_ref_audio)
                .hint_text("Optional dedicated reference WAV for the agent's own voice"),
        );
    });
    if !model.agent_voice.director_ref_audio.trim().is_empty() {
        ui.add_sized(
            [ui.available_width(), 24.0],
            egui::TextEdit::singleline(&mut model.agent_voice.director_ref_text)
                .hint_text("Exact spoken words of the agent's reference WAV"),
        );
        ui.horizontal_wrapped(|ui| {
            ui.label("Agent Language");
            egui::ComboBox::from_id_salt("director_language")
                .width(80.0)
                .selected_text(&model.agent_voice.director_language)
                .show_ui(ui, |ui| {
                    for &lang in reverb_tts::text::SUPPORTED_LANGUAGES {
                        ui.selectable_value(
                            &mut model.agent_voice.director_language,
                            lang.to_owned(),
                            lang,
                        );
                    }
                });
        });
    } else {
        ui.horizontal_wrapped(|ui| {
            ui.label("Agent Preset Voice");
            ui.add(
                egui::TextEdit::singleline(&mut model.agent_voice.director_voice_preset)
                    .desired_width(190.0)
                    .hint_text("Inherit main narrator voice"),
            );
            if ui.small_button("Voice JSON…").clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .add_filter("Voice JSON", &["json"])
                    .pick_file()
            {
                model.agent_voice.director_voice_preset = path.display().to_string();
            }
            if !model.agent_voice.director_voice_preset.trim().is_empty()
                && ui.small_button("Clear").clicked()
            {
                model.agent_voice.director_voice_preset.clear();
            }
        });
    }

    ui.add_space(12.0);
    ui.separator();
    ui.add_space(8.0);
    ui.heading("Local Agent Harness (Loopback Only)");
    ui.colored_label(
        theme::text_muted(),
        "Expose an opt-in local HTTP/SSE API bound strictly to 127.0.0.1 so external local agents \
         (e.g. Hermes or custom scripts) can inspect live state, events, webcams, and screenshots, \
         and take full control of camera, layers, annotations, and narration.",
    );
    ui.add_space(6.0);
    ui.checkbox(
        &mut model.agent_voice.agent_harness_enabled,
        "Enable local agent bridge on 127.0.0.1",
    );
    ui.horizontal_wrapped(|ui| {
        ui.label("Port");
        ui.add(
            egui::DragValue::new(&mut model.agent_voice.agent_harness_port)
                .range(1024..=65535),
        );
        ui.small(format!("Status: {}", model.agent_harness_status));
    });
    ui.add_space(6.0);
    let port = model.agent_voice.agent_harness_port;
    ui.monospace(format!("GET  http://127.0.0.1:{port}/v1/state"));
    ui.monospace(format!("GET  http://127.0.0.1:{port}/v1/events"));
    ui.monospace(format!("GET  http://127.0.0.1:{port}/v1/cameras"));
    ui.monospace(format!("GET  http://127.0.0.1:{port}/v1/screenshot"));
    ui.monospace(format!("GET  http://127.0.0.1:{port}/v1/capabilities"));
    ui.monospace(format!("GET  http://127.0.0.1:{port}/v1/events/stream"));
    ui.monospace(format!("POST http://127.0.0.1:{port}/v1/camera"));
    ui.monospace(format!("POST http://127.0.0.1:{port}/v1/layers"));
    ui.monospace(format!("POST http://127.0.0.1:{port}/v1/narrate"));
    ui.monospace(format!("POST http://127.0.0.1:{port}/v1/annotate"));
    ui.monospace(format!("POST http://127.0.0.1:{port}/v1/annotations/clear"));
    ui.add_space(6.0);
    ui.small("Use Save Settings to remember your Voice, AI Agent Director, and Loopback Harness preferences.");
}
