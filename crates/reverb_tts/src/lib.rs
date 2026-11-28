//! Pure-Rust Reverb-TTS runner (Qwen3 masked-diffusion + HiggsAudioV2 DAC decoder).
//!
//! Zero Python, zero sidecar servers, zero containers: loads standard HuggingFace
//! `.safetensors` checkpoints (`int4` or `fp16`), `tokenizer.json`, pre-encoded
//! `voices/*.json` presets, and the decoder-only tensors of `higgs-audio-v2-tokenizer`.

pub mod codec;
pub mod encoder;
pub mod model;
pub mod sampler;
pub mod text;

use candle_core::{DType, Device};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokenizers::Tokenizer;

pub use codec::SAMPLE_RATE;
pub use sampler::SamplerConfig;

const FRAME_RATE: usize = 25;
const LONGEST_SECS: usize = 30;
const PIECE_SECS: usize = 15;
const END_CUT_SECS: f32 = 0.2;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ComputeDevice {
    /// Use CUDA GPU by ordinal (e.g. `0` = RTX 2070 SUPER, `1` = RTX 4090), falling back to CPU.
    Cuda(usize),
    #[default]
    Cpu,
}

impl ComputeDevice {
    pub fn resolve(self) -> Result<(Device, DType, String), String> {
        match self {
            Self::Cuda(ordinal) => {
                #[cfg(feature = "cuda")]
                {
                    let dev = Device::new_cuda(ordinal)
                        .map_err(|e| format!("CUDA device {ordinal} unavailable: {e}"))?;
                    // F16 runs natively on both Turing (2070S) and Ada (4090) Tensor Cores.
                    Ok((dev, DType::F16, format!("CUDA:{ordinal} (fp16)")))
                }
                #[cfg(not(feature = "cuda"))]
                {
                    let _ = ordinal;
                    Err("Built without CUDA support; enable the `reverb-cuda` feature or select CPU.".into())
                }
            }
            Self::Cpu => Ok((Device::Cpu, DType::F32, "CPU (fp32)".into())),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Voice {
    pub name: String,
    pub language: String,
    pub transcript: String,
    /// Row-major `[8, frames]` codec indices in `0..1024`.
    pub codes: Vec<u32>,
    pub frames: usize,
    pub speech_frames: usize,
}

impl Voice {
    pub fn load_json(path: &Path) -> Result<Self, String> {
        #[derive(Deserialize)]
        struct VoiceJson {
            #[serde(default)]
            name: String,
            language: String,
            transcript: String,
            codes: Vec<u32>,
            #[serde(default)]
            speech_frames: Option<usize>,
        }
        let raw = std::fs::read_to_string(path)
            .map_err(|e| format!("Cannot read voice {}: {e}", path.display()))?;
        let parsed: VoiceJson = serde_json::from_str(&raw)
            .map_err(|e| format!("Invalid voice JSON {}: {e}", path.display()))?;
        text::check_language(&parsed.language)?;
        if parsed.codes.is_empty() || parsed.codes.len() % model::CODEBOOKS != 0 {
            return Err(format!(
                "{}: expected codes divisible by {}",
                path.display(),
                model::CODEBOOKS
            ));
        }
        if parsed.codes.iter().any(|&c| c >= model::MASK) {
            return Err(format!("{}: code value out of range", path.display()));
        }
        let frames = parsed.codes.len() / model::CODEBOOKS;
        let speech_frames = parsed
            .speech_frames
            .map(|sf| sf.clamp(1, frames))
            .unwrap_or(frames);
        let name = if parsed.name.trim().is_empty() {
            path.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("voice")
                .to_owned()
        } else {
            parsed.name
        };
        Ok(Self {
            name,
            language: parsed.language,
            transcript: parsed.transcript,
            codes: parsed.codes,
            frames,
            speech_frames,
        })
    }

    /// Save an encoded voice to a Reverb-compatible `.json` file so it never needs re-encoding.
    pub fn save_json(&self, path: &Path) -> Result<(), String> {
        #[derive(Serialize)]
        struct VoiceOut<'a> {
            name: &'a str,
            language: &'a str,
            transcript: &'a str,
            frames: usize,
            duration: f32,
            speech_frames: usize,
            codes: &'a [u32],
        }
        let payload = VoiceOut {
            name: &self.name,
            language: &self.language,
            transcript: &self.transcript,
            frames: self.frames,
            duration: self.frames as f32 / FRAME_RATE as f32,
            speech_frames: self.speech_frames,
            codes: &self.codes,
        };
        let json = serde_json::to_string(&payload).map_err(|e| e.to_string())?;
        std::fs::write(path, json).map_err(|e| format!("Cannot save {}: {e}", path.display()))
    }
}

/// Resolved directories under a ReverbTTS checkpoints root.
#[derive(Clone, Debug)]
pub struct ReverbPaths {
    pub weights_dir: PathBuf,
    pub tokenizer_path: PathBuf,
    pub codec_dir: PathBuf,
    pub voices_dir: Option<PathBuf>,
    pub variant_label: String,
}

/// Resolve model, tokenizer, codec, and voice directories from either:
/// - a parent checkpoints root containing `Reverb-TTS/` and `higgs-audio-v2-tokenizer/`
/// - a `Reverb-TTS` directory whose sibling is `higgs-audio-v2-tokenizer/`
pub fn resolve_paths(root: &Path, prefer_int4: bool) -> Result<ReverbPaths, String> {
    let reverb_root = if root.join("Reverb-TTS").is_dir() {
        root.join("Reverb-TTS")
    } else if root.join("checkpoints/Reverb-TTS").is_dir() {
        root.join("checkpoints/Reverb-TTS")
    } else {
        root.to_path_buf()
    };

    let variants: &[&str] = if prefer_int4 {
        &["int4", "int8", "fp16", "fp32"]
    } else {
        &["fp16", "int4", "int8", "fp32"]
    };
    let mut found_weights: Option<(PathBuf, String)> = None;
    for &v in variants {
        for cand in [
            reverb_root.join("model/safetensors").join(v),
            reverb_root.join("model").join(v),
            reverb_root.join(v),
        ] {
            if cand.join("config.json").is_file() && cand.join("model.safetensors").is_file() {
                found_weights = Some((cand, v.to_owned()));
                break;
            }
        }
        if found_weights.is_some() {
            break;
        }
    }
    if found_weights.is_none()
        && reverb_root.join("config.json").is_file()
        && reverb_root.join("model.safetensors").is_file()
    {
        found_weights = Some((reverb_root.clone(), "direct".into()));
    }
    let (weights_dir, variant_label) = found_weights.ok_or_else(|| {
        format!(
            "No Reverb-TTS checkpoint (config.json + model.safetensors) found under {}",
            root.display()
        )
    })?;

    let tokenizer_path = [
        weights_dir.join("tokenizer.json"),
        reverb_root.join("model/tokenizer.json"),
        reverb_root.join("tokenizer.json"),
    ]
    .into_iter()
    .find(|p| p.is_file())
    .ok_or_else(|| format!("No tokenizer.json found under {}", reverb_root.display()))?;

    let codec_dir = [
        root.join("higgs-audio-v2-tokenizer"),
        root.join("checkpoints/higgs-audio-v2-tokenizer"),
        reverb_root
            .parent()
            .map(|p| p.join("higgs-audio-v2-tokenizer"))
            .unwrap_or_default(),
        reverb_root.join("higgs-audio-v2-tokenizer"),
    ]
    .into_iter()
    .find(|p| p.join("model.safetensors").is_file())
    .ok_or_else(|| {
        format!(
            "No higgs-audio-v2-tokenizer/model.safetensors found beside {}",
            reverb_root.display()
        )
    })?;

    let voices_dir = [reverb_root.join("voices"), root.join("voices")]
        .into_iter()
        .find(|p| p.is_dir());

    Ok(ReverbPaths {
        weights_dir,
        tokenizer_path,
        codec_dir,
        voices_dir,
        variant_label,
    })
}

/// Discover sorted preset voice names (`*.json`) under a ReverbTTS root.
pub fn list_preset_voices(root: &Path) -> Vec<String> {
    let Ok(paths) = resolve_paths(root, true) else {
        return Vec::new();
    };
    let Some(dir) = paths.voices_dir else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("json"))
        .filter_map(|p| p.file_stem().and_then(|s| s.to_str()).map(ToOwned::to_owned))
        .collect();
    names.sort();
    names
}

pub struct ReverbEngine {
    network: model::ReverbNetwork,
    decoder: codec::HiggsDecoder,
    tokenizer: Tokenizer,
    paths: ReverbPaths,
    device_label: String,
    sampler: SamplerConfig,
}

impl ReverbEngine {
    pub fn load(root: &Path, prefer_int4: bool, compute: ComputeDevice) -> Result<Self, String> {
        let paths = resolve_paths(root, prefer_int4)?;
        let (device, dtype, device_label) = compute.resolve()?;
        let tokenizer = Tokenizer::from_file(&paths.tokenizer_path)
            .map_err(|e| format!("Cannot load tokenizer {}: {e}", paths.tokenizer_path.display()))?;
        let network = model::ReverbNetwork::load(&paths.weights_dir, &device, dtype)?;
        let decoder = codec::HiggsDecoder::load(&paths.codec_dir, &device)?;
        Ok(Self {
            network,
            decoder,
            tokenizer,
            paths,
            device_label,
            sampler: SamplerConfig::default(),
        })
    }

    pub fn device_label(&self) -> &str {
        &self.device_label
    }

    pub fn variant_label(&self) -> &str {
        &self.paths.variant_label
    }

    pub fn load_preset_voice(&self, name: &str) -> Result<Voice, String> {
        let dir = self
            .paths
            .voices_dir
            .as_ref()
            .ok_or("No Reverb-TTS voices/ directory found")?;
        let path = dir.join(format!("{name}.json"));
        Voice::load_json(&path)
    }

    /// Zero-shot voice cloning from any `.wav` file + its exact transcript.
    /// Loads the HiggsAudioV2 encoder weights on demand and drops them immediately
    /// after encoding so zero extra GPU VRAM stays resident.
    pub fn clone_voice_from_wav(
        &self,
        wav_path: &Path,
        transcript: &str,
        language: &str,
    ) -> Result<Voice, String> {
        let enc = encoder::HiggsEncoder::load(&self.paths.codec_dir, &self.network.device)?;
        enc.encode_wav_file(wav_path, transcript, language)
    }

    /// Synthesize speech into mono 24 kHz float32 samples (`[-1.0, 1.0]`).
    pub fn generate(&self, input_text: &str, voice: &Voice, seed: u64) -> Result<Vec<f32>, String> {
        let lang = text::check_language(&voice.language)?;
        let prepared = text::prepare_text(input_text, lang);
        if prepared.is_empty() {
            return Err("Empty text for ReverbTTS synthesis".into());
        }
        let pieces = text::split_text(
            &prepared,
            &voice.transcript,
            voice.speech_frames,
            LONGEST_SECS * FRAME_RATE,
            PIECE_SECS * FRAME_RATE,
        );
        let mut combined = Vec::new();
        for (i, piece) in pieces.iter().enumerate() {
            let frames = text::estimate_frames(piece, &voice.transcript, voice.speech_frames);
            let prompt = text::format_prompt(piece, &voice.transcript, lang);
            let enc = self
                .tokenizer
                .encode(prompt, true)
                .map_err(|e| format!("Tokenization failed: {e}"))?;
            let codes = sampler::sample_take(
                &self.network,
                enc.get_ids(),
                &voice.codes,
                voice.frames,
                frames,
                seed.wrapping_add(i as u64),
                &self.sampler,
            )?;
            let mut audio = self.decoder.decode(&codes, frames)?;
            let cut = (END_CUT_SECS * SAMPLE_RATE as f32) as usize;
            if audio.len() > cut {
                audio.truncate(audio.len() - cut);
            }
            let trimmed = text::trim_silence(&audio, SAMPLE_RATE, 0.1);
            combined.extend_from_slice(&trimmed);
        }
        Ok(combined)
    }

    /// Synthesize directly to a 16-bit mono 24 kHz RIFF/WAVE byte buffer.
    pub fn generate_wav(&self, input_text: &str, voice: &Voice, seed: u64) -> Result<Vec<u8>, String> {
        let samples = self.generate(input_text, voice, seed)?;
        Ok(samples_to_wav(&samples, SAMPLE_RATE as u32))
    }
}

pub fn samples_to_wav(samples: &[f32], sample_rate: u32) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let byte_rate = sample_rate * 2;
    let mut wav = Vec::with_capacity(44 + data_len as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36u32.saturating_add(data_len)).to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&byte_rate.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    for &s in samples {
        let q = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        wav.extend_from_slice(&q.to_le_bytes());
    }
    wav
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_local_reverb_checkpoints_when_present() {
        let root = PathBuf::from("/run/media/m/Minkowski/Models/ReverbTTS");
        if !root.is_dir() {
            return;
        }
        let paths = resolve_paths(&root, true).expect("resolve local checkpoints");
        assert_eq!(paths.variant_label, "int4");
        let presets = list_preset_voices(&root);
        assert!(presets.iter().any(|p| p == "en_male_narrator_1"));
    }

    #[test]
    #[ignore = "live GPU synthesis smoke test against local Reverb-TTS checkpoints"]
    fn live_cuda_int4_synthesis_produces_audible_speech() {
        let root = PathBuf::from("/run/media/m/Minkowski/Models/ReverbTTS");
        let t0 = std::time::Instant::now();
        let engine = ReverbEngine::load(&root, true, ComputeDevice::Cuda(0))
            .expect("load ReverbEngine on CUDA:0");
        let load_s = t0.elapsed().as_secs_f32();
        let voice = engine
            .load_preset_voice("en_male_narrator_1")
            .expect("load preset voice");
        let _ = engine.generate("Warm up.", &voice, 0).expect("warmup");
        let t1 = std::time::Instant::now();
        let wav = engine
            .generate_wav(
                "One Thousand Electric Eye embedded narrator online. Magnitude five point four earthquake detected near Tokyo.",
                &voice,
                42,
            )
            .expect("synthesize wav");
        let gen_s = t1.elapsed().as_secs_f32();
        let audio_s = (wav.len().saturating_sub(44)) as f32 / (2.0 * SAMPLE_RATE as f32);
        println!(
            "ReverbEngine ({} / {}) load={load_s:.2}s gen={gen_s:.3}s audio={audio_s:.2}s rtf={:.3}",
            engine.variant_label(),
            engine.device_label(),
            gen_s / audio_s.max(0.01)
        );
        assert!(audio_s > 1.5, "audio too short: {audio_s}s");
        std::fs::write("/tmp/reverb_rust_smoke.wav", &wav).unwrap();
    }

    #[test]
    #[ignore = "live GPU zero-shot WAV voice cloning smoke test"]
    fn live_cuda_int4_voice_cloning_from_wav_produces_audible_speech() {
        let root = PathBuf::from("/run/media/m/Minkowski/Models/ReverbTTS");
        let ref_wav = PathBuf::from("/home/m/Code/BreezeTTS/deployment/voices/tris-reference.wav");
        let ref_text = "Hi Max. It's Tris. I'm trying out a new voice on the little graphics card.";
        let engine = ReverbEngine::load(&root, true, ComputeDevice::Cuda(0))
            .expect("load ReverbEngine on CUDA:0");
        let tc = std::time::Instant::now();
        let cloned = engine
            .clone_voice_from_wav(&ref_wav, ref_text, "en")
            .expect("clone voice from wav");
        let clone_s = tc.elapsed().as_secs_f32();
        println!(
            "Cloned voice '{}' in {clone_s:.2}s: {} frames ({} speech frames)",
            cloned.name, cloned.frames, cloned.speech_frames
        );
        let t1 = std::time::Instant::now();
        let wav = engine
            .generate_wav(
                "Zero shot voice cloning in pure Rust is online and running directly inside One Thousand Electric Eye.",
                &cloned,
                42,
            )
            .expect("synthesize cloned wav");
        let gen_s = t1.elapsed().as_secs_f32();
        let audio_s = (wav.len().saturating_sub(44)) as f32 / (2.0 * SAMPLE_RATE as f32);
        println!("Cloned synthesis: gen={gen_s:.3}s audio={audio_s:.2}s");
        assert!(audio_s > 2.0, "cloned audio too short: {audio_s}s");
        std::fs::write("/tmp/reverb_rust_cloned.wav", &wav).unwrap();
    }
}
