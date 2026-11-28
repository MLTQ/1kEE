//! Pure-Rust zero-shot voice cloning encoder for Reverb-TTS (`HiggsAudioV2TokenizerModel.encode`).
//!
//! Reads any PCM/float `.wav` file, resamples with a Kaiser-windowed sinc filter
//! to 24 kHz (acoustic DAC encoder) and 16 kHz (12-layer HuBERT + semantic encoder),
//! fuses features via `fc`, and quantizes into 8-codebook discrete tokens.
//!
//! Loaded on demand only when encoding a new WAV file and dropped immediately
//! afterward so it occupies zero resident GPU VRAM during normal narration.

use candle_core::{D, DType, Device, Result, Tensor};
use candle_nn::ops::softmax_last_dim;
use safetensors::{Dtype as StDtype, SafeTensors};
use std::path::Path;

use crate::model::CODEBOOKS;
use crate::{Voice, text};

const ACOUSTIC_SR: usize = 24_000;
const SEMANTIC_SR: usize = 16_000;
const HOP_LENGTH: usize = 960;
const FRAME_RATE: usize = 25;
const DOWNSAMPLING_RATIOS: [usize; 5] = [8, 5, 4, 2, 3];

// ── Pure-Rust WAV reader & audio conditioning ─────────────────────────────────

pub fn read_wav_mono_f32(path: &Path) -> std::result::Result<(Vec<f32>, usize), String> {
    let data = std::fs::read(path)
        .map_err(|e| format!("Cannot read WAV file {}: {e}", path.display()))?;
    if data.len() < 44 || &data[0..4] != b"RIFF" || &data[8..12] != b"WAVE" {
        return Err(format!("{} is not a valid RIFF/WAVE file", path.display()));
    }
    let mut pos = 12usize;
    let mut audio_format = 0u16;
    let mut channels = 0u16;
    let mut sample_rate = 0u32;
    let mut bits_per_sample = 0u16;
    let mut pcm_bytes: Option<&[u8]> = None;

    while pos + 8 <= data.len() {
        let chunk_id = &data[pos..pos + 4];
        let chunk_size = u32::from_le_bytes([
            data[pos + 4],
            data[pos + 5],
            data[pos + 6],
            data[pos + 7],
        ]) as usize;
        pos += 8;
        let end = (pos + chunk_size).min(data.len());
        if chunk_id == b"fmt " && end - pos >= 16 {
            audio_format = u16::from_le_bytes([data[pos], data[pos + 1]]);
            channels = u16::from_le_bytes([data[pos + 2], data[pos + 3]]);
            sample_rate = u32::from_le_bytes([
                data[pos + 4],
                data[pos + 5],
                data[pos + 6],
                data[pos + 7],
            ]);
            bits_per_sample = u16::from_le_bytes([data[pos + 14], data[pos + 15]]);
            // WAVE_FORMAT_EXTENSIBLE: subformat code is first 2 bytes at offset 24
            if audio_format == 0xFFFE && end - pos >= 26 {
                audio_format = u16::from_le_bytes([data[pos + 24], data[pos + 25]]);
            }
        } else if chunk_id == b"data" {
            pcm_bytes = Some(&data[pos..end]);
        }
        pos = end + (chunk_size & 1); // RIFF chunks are word-aligned
    }

    let raw = pcm_bytes.ok_or_else(|| format!("WAV {} has no data chunk", path.display()))?;
    if channels == 0 || sample_rate == 0 {
        return Err(format!("WAV {} has invalid fmt header", path.display()));
    }
    let ch = channels as usize;
    let bytes_per_sample = (bits_per_sample as usize) / 8;
    if bytes_per_sample == 0 {
        return Err(format!("Unsupported WAV bit depth {bits_per_sample}"));
    }
    let frame_bytes = ch * bytes_per_sample;
    let num_frames = raw.len() / frame_bytes;
    let mut mono = Vec::with_capacity(num_frames);

    for f in 0..num_frames {
        let mut acc = 0.0_f32;
        for c in 0..ch {
            let off = f * frame_bytes + c * bytes_per_sample;
            let s = match (audio_format, bits_per_sample) {
                (1, 16) => i16::from_le_bytes([raw[off], raw[off + 1]]) as f32 / 32768.0,
                (1, 24) => {
                    let b0 = raw[off] as i32;
                    let b1 = (raw[off + 1] as i32) << 8;
                    let b2 = ((raw[off + 2] as i8) as i32) << 16;
                    (b0 | b1 | b2) as f32 / 8_388_608.0
                }
                (1, 32) => {
                    i32::from_le_bytes([raw[off], raw[off + 1], raw[off + 2], raw[off + 3]]) as f32
                        / 2_147_483_648.0
                }
                (3, 32) => f32::from_le_bytes([raw[off], raw[off + 1], raw[off + 2], raw[off + 3]]),
                _ => {
                    return Err(format!(
                        "Unsupported WAV format={audio_format}, bits={bits_per_sample} (expected 16/24/32-bit PCM or 32-bit float)"
                    ));
                }
            };
            acc += s;
        }
        mono.push(acc / ch as f32);
    }
    Ok((mono, sample_rate as usize))
}

fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let half_x_sq = (x * 0.5) * (x * 0.5);
    for k in 1..25 {
        term *= half_x_sq / ((k * k) as f64);
        sum += term;
        if term < 1e-12 * sum {
            break;
        }
    }
    sum
}

/// Kaiser-windowed sinc resampler matching librosa/torchaudio `kaiser_best` parameters.
pub fn resample_kaiser(input: &[f32], src_sr: usize, dst_sr: usize) -> Vec<f32> {
    if src_sr == dst_sr || input.is_empty() {
        return input.to_vec();
    }
    const ROLLOFF: f64 = 0.9475937167399596;
    const BETA: f64 = 14.769656459379392;
    const HALF_WIDTH: i64 = 32;

    let ratio = dst_sr as f64 / src_sr as f64;
    let out_len = ((input.len() as f64) * ratio).ceil() as usize;
    let cutoff = ROLLOFF * ratio.min(1.0);
    let scale = cutoff;
    let inv_i0_beta = 1.0 / bessel_i0(BETA);
    let support = (HALF_WIDTH as f64 / ratio.min(1.0)).ceil() as i64;
    let max_dist = support as f64;

    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let center = (i as f64) / ratio;
        let center_i = center.floor() as i64;
        let j_start = (center_i - support).max(0);
        let j_end = (center_i + support).min(input.len() as i64 - 1);
        let mut acc = 0.0_f64;
        for j in j_start..=j_end {
            let dt = (j as f64) - center;
            let norm_t = dt / max_dist;
            if norm_t.abs() >= 1.0 {
                continue;
            }
            let win = bessel_i0(BETA * (1.0 - norm_t * norm_t).sqrt()) * inv_i0_beta;
            let x = std::f64::consts::PI * cutoff * dt;
            let sinc = if x.abs() < 1e-9 { 1.0 } else { x.sin() / x };
            acc += (input[j as usize] as f64) * scale * sinc * win;
        }
        out.push(acc as f32);
    }
    out
}

/// Exact port of `reverb_tts/audio.py:raise_quiet`.
fn raise_quiet(mut audio: Vec<f32>) -> Vec<f32> {
    if audio.is_empty() {
        return audio;
    }
    let rms = (audio.iter().map(|&s| (s as f64) * (s as f64)).sum::<f64>() / audio.len() as f64)
        .sqrt() as f32;
    if rms <= 0.0 || 20.0 * rms.log10() >= -40.0 {
        return audio;
    }
    let peak = audio.iter().fold(0.0_f32, |m, &s| m.max(s.abs())).max(1e-8);
    let gain = (10.0_f32.powf(-20.0 / 20.0) / rms).min(0.99 / peak);
    for s in &mut audio {
        *s *= gain;
    }
    audio
}

/// Exact port of `reverb_tts/audio.py:speech_length` (returns speaking duration in seconds).
fn speech_length_secs(audio: &[f32], sample_rate: usize) -> f32 {
    const SILENCE_DB: f64 = -50.0;
    let window = (sample_rate / 100).max(1);
    let count = audio.len() / window;
    if count == 0 {
        return audio.len() as f32 / sample_rate as f32;
    }
    let n = count * window;
    let rms = (audio[..n]
        .iter()
        .map(|&s| (s as f64) * (s as f64))
        .sum::<f64>()
        / n as f64)
        .sqrt()
        .max(1e-8);
    let norm = 0.1 / rms;

    let mut loud = Vec::new();
    for w in 0..count {
        let mean_sq = audio[w * window..(w + 1) * window]
            .iter()
            .map(|&s| {
                let v = (s as f64) * norm;
                v * v
            })
            .sum::<f64>()
            / window as f64;
        let db = 20.0 * (mean_sq.sqrt() + 1e-10).log10();
        if db >= SILENCE_DB {
            loud.push(w);
        }
    }
    if loud.is_empty() {
        return audio.len() as f32 / sample_rate as f32;
    }
    let first = loud[0];
    let last = *loud.last().unwrap();
    let max_pause_w = 50usize; // 0.5s
    let mut kept = loud.len();
    for pair in loud.windows(2) {
        let gap = pair[1].saturating_sub(pair[0] + 1);
        kept += gap.min(max_pause_w);
    }
    let edges = first.min(10) + (count - 1 - last).min(20);
    (kept + edges) as f32 / 100.0
}

// ── Building blocks ───────────────────────────────────────────────────────────

struct Snake1d {
    alpha: Tensor, // [1, C, 1]
}
impl Snake1d {
    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        let sin_sq = x.broadcast_mul(&self.alpha)?.sin()?.sqr()?;
        let denom = (&self.alpha + 1e-9)?;
        x + sin_sq.broadcast_div(&denom)?
    }
}

struct Conv1d {
    weight: Tensor,
    bias: Option<Tensor>, // [1, out, 1]
    stride: usize,
    padding: usize,
    dilation: usize,
    groups: usize,
}
impl Conv1d {
    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        let y = x.conv1d(&self.weight, self.padding, self.stride, self.dilation, self.groups)?;
        if let Some(b) = &self.bias {
            y.broadcast_add(b)
        } else {
            Ok(y)
        }
    }
}

struct Linear {
    weight_t: Tensor,
    bias: Tensor, // [1, out]
}
impl Linear {
    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        match x.dims() {
            [b, m, k] => {
                let out_dim = self.weight_t.dim(1)?;
                x.reshape((b * m, *k))?
                    .matmul(&self.weight_t)?
                    .broadcast_add(&self.bias)?
                    .reshape((*b, *m, out_dim))
            }
            _ => x.matmul(&self.weight_t)?.broadcast_add(&self.bias),
        }
    }
}

struct LayerNorm {
    weight: Tensor, // [D]
    bias: Tensor,   // [D]
    eps: f64,
}
impl LayerNorm {
    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        let mean = x.mean_keepdim(D::Minus1)?;
        let centered = x.broadcast_sub(&mean)?;
        let var = centered.sqr()?.mean_keepdim(D::Minus1)?;
        let normed = centered.broadcast_div(&(var + self.eps)?.sqrt()?)?;
        normed
            .broadcast_mul(&self.weight)?
            .broadcast_add(&self.bias)
    }
}

fn elu(x: &Tensor) -> Result<Tensor> {
    // ELU(x) = x if x > 0 else exp(x) - 1
    let zero = Tensor::zeros(x.shape(), x.dtype(), x.device())?;
    let pos = x.maximum(&zero)?;
    let neg = (x.minimum(&zero)?.exp()? - 1.0)?;
    pos + neg
}

// ── Acoustic DAC Encoder ──────────────────────────────────────────────────────

struct DacResUnit {
    snake1: Snake1d,
    conv1: Conv1d,
    snake2: Snake1d,
    conv2: Conv1d,
}
impl DacResUnit {
    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        let h = self.conv1.forward(&self.snake1.forward(x)?)?;
        let h = self.conv2.forward(&self.snake2.forward(&h)?)?;
        let x_len = x.dim(2)?;
        let h_len = h.dim(2)?;
        if x_len > h_len {
            let pad = (x_len - h_len) / 2;
            x.narrow(2, pad, h_len)? + h
        } else {
            x + h
        }
    }
}

struct DacEncoderBlock {
    res_units: [DacResUnit; 3],
    snake1: Snake1d,
    conv1: Conv1d,
}
impl DacEncoderBlock {
    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        let mut h = x.clone();
        for unit in &self.res_units {
            h = unit.forward(&h)?;
        }
        self.conv1.forward(&self.snake1.forward(&h)?)
    }
}

struct DacEncoder {
    conv1: Conv1d,
    blocks: Vec<DacEncoderBlock>,
    snake1: Snake1d,
    conv2: Conv1d,
}
impl DacEncoder {
    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        let mut h = self.conv1.forward(x)?;
        for b in &self.blocks {
            h = b.forward(&h)?;
        }
        self.conv2.forward(&self.snake1.forward(&h)?)
    }
}

// ── HuBERT Semantic Model + SemanticEncoder ───────────────────────────────────

struct HubertEncoderLayer {
    q_proj: Linear,
    k_proj: Linear,
    v_proj: Linear,
    out_proj: Linear,
    layer_norm: LayerNorm,
    intermediate_dense: Linear,
    output_dense: Linear,
    final_layer_norm: LayerNorm,
}
impl HubertEncoderLayer {
    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        let (b, t, d) = x.dims3()?;
        let heads = 12usize;
        let hd = d / heads; // 64
        let q = self
            .q_proj
            .forward(x)?
            .reshape((b, t, heads, hd))?
            .transpose(1, 2)?
            .contiguous()?;
        let k = self
            .k_proj
            .forward(x)?
            .reshape((b, t, heads, hd))?
            .transpose(1, 2)?
            .contiguous()?;
        let v = self
            .v_proj
            .forward(x)?
            .reshape((b, t, heads, hd))?
            .transpose(1, 2)?
            .contiguous()?;
        let kt = k.transpose(2, 3)?.contiguous()?;
        let scores = (q.matmul(&kt)? * (1.0 / (hd as f64).sqrt()))?;
        let probs = softmax_last_dim(&scores)?;
        let ctx = probs
            .matmul(&v)?
            .transpose(1, 2)?
            .contiguous()?
            .reshape((b, t, d))?;
        let attn = self.out_proj.forward(&ctx)?;
        let x = self.layer_norm.forward(&(x + attn)?)?;
        let ff = self
            .output_dense
            .forward(&self.intermediate_dense.forward(&x)?.gelu_erf()?)?;
        self.final_layer_norm.forward(&(x + ff)?)
    }
}

struct HubertSemanticModel {
    feat_conv0: Conv1d,
    feat_gn0_w: Tensor, // [1, 512, 1]
    feat_gn0_b: Tensor, // [1, 512, 1]
    feat_convs: Vec<Conv1d>,
    proj_ln: LayerNorm,
    proj_linear: Linear,
    pos_conv: Conv1d,
    enc_ln: LayerNorm,
    layers: Vec<HubertEncoderLayer>,
}
impl HubertSemanticModel {
    fn forward(&self, wav_16k: &Tensor) -> Result<Tensor> {
        // wav_16k: [1, 1, N] (already padded by (160, 160))
        let mut h = self.feat_conv0.forward(wav_16k)?; // [1, 512, T0]
        // GroupNorm(512, 512) == per-channel InstanceNorm over dim 2
        let mean = h.mean_keepdim(2)?;
        let centered = h.broadcast_sub(&mean)?;
        let var = centered.sqr()?.mean_keepdim(2)?;
        h = centered
            .broadcast_div(&(var + 1e-5)?.sqrt()?)?
            .broadcast_mul(&self.feat_gn0_w)?
            .broadcast_add(&self.feat_gn0_b)?
            .gelu_erf()?;
        for conv in &self.feat_convs {
            h = conv.forward(&h)?.gelu_erf()?;
        }
        // [1, 512, T] -> [1, T, 512] -> [1, T, 768]
        let h = h.transpose(1, 2)?.contiguous()?;
        let x = self.proj_linear.forward(&self.proj_ln.forward(&h)?)?;
        let t_len = x.dim(1)?;

        // Positional conv embedding (SamePadLayer drops 1 trailing sample because k=128 is even)
        let pos = self
            .pos_conv
            .forward(&x.transpose(1, 2)?.contiguous()?)?
            .narrow(2, 0, t_len)?
            .gelu_erf()?
            .transpose(1, 2)?
            .contiguous()?;
        let mut cur = self.enc_ln.forward(&(x + pos)?)?;

        let mut sum = cur.clone();
        for layer in &self.layers {
            cur = layer.forward(&cur)?;
            sum = (sum + &cur)?;
        }
        // Mean of all 13 hidden states (input + 12 layers)
        let mean_states = (sum / ((self.layers.len() + 1) as f64))?;
        // Subsample by semantic_downsample_factor = 2 ([::2])
        let out_t = t_len.div_ceil(2);
        let idxs: Vec<u32> = (0..out_t).map(|i| (i * 2) as u32).collect();
        let idx_t = Tensor::from_vec(idxs, out_t, mean_states.device())?;
        mean_states.squeeze(0)?.index_select(&idx_t, 0)?.unsqueeze(0)
    }
}

struct SemanticResUnit {
    conv1: Conv1d,
    conv2: Conv1d,
}
impl SemanticResUnit {
    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        let h = self.conv1.forward(&elu(x)?)?;
        let h = self.conv2.forward(&elu(&h)?)?;
        x + h
    }
}

struct SemanticEncoderBlock {
    res_units: [SemanticResUnit; 2],
    conv: Conv1d,
}
impl SemanticEncoderBlock {
    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        let mut h = x.clone();
        for u in &self.res_units {
            h = u.forward(&h)?;
        }
        self.conv.forward(&h)
    }
}

struct SemanticEncoder {
    conv: Conv1d,
    blocks: [SemanticEncoderBlock; 2],
}
impl SemanticEncoder {
    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        let mut h = self.conv.forward(x)?;
        for b in &self.blocks {
            h = b.forward(&h)?;
        }
        Ok(h)
    }
}

// ── RVQ Encoder ───────────────────────────────────────────────────────────────

struct RvqEncoderLayer {
    project_in: Linear,  // 1024 -> 64
    project_out: Linear, // 64 -> 1024
    embed: Tensor,       // [1024, 64]
    embed_sq: Tensor,    // [1, 1024]
}
impl RvqEncoderLayer {
    fn encode_step(&self, residual: &Tensor) -> Result<(Vec<u32>, Tensor)> {
        // residual: [F, 1024]
        let proj = self.project_in.forward(residual)?; // [F, 64]
        let proj_sq = proj.sqr()?.sum_keepdim(1)?; // [F, 1]
        let dot = proj.matmul(&self.embed.t()?.contiguous()?)?; // [F, 1024]
        // Maximize -(||x||^2 - 2 x.e + ||e||^2) <=> minimize Euclidean distance
        let neg_dist = (dot * 2.0)?
            .broadcast_sub(&proj_sq)?
            .broadcast_sub(&self.embed_sq)?;
        let indices_t = neg_dist.argmax(1)?; // [F]
        let indices: Vec<u32> = indices_t.to_dtype(DType::U32)?.to_vec1()?;
        let q = self.embed.index_select(&indices_t, 0)?; // [F, 64]
        let quantized = self.project_out.forward(&q)?; // [F, 1024]
        let next_residual = (residual - quantized)?;
        Ok((indices, next_residual))
    }
}

// ── Complete On-Demand Voice Cloning Encoder ──────────────────────────────────

pub struct HiggsEncoder {
    acoustic_encoder: DacEncoder,
    semantic_model: HubertSemanticModel,
    encoder_semantic: SemanticEncoder,
    fc: Linear,
    quantizers: Vec<RvqEncoderLayer>,
    device: Device,
}

impl HiggsEncoder {
    pub fn load(codec_dir: &Path, device: &Device) -> std::result::Result<Self, String> {
        let path = codec_dir.join("model.safetensors");
        let bytes = std::fs::read(&path)
            .map_err(|e| format!("Cannot read HiggsAudioV2 codec {}: {e}", path.display()))?;
        let st = SafeTensors::deserialize(&bytes)
            .map_err(|e| format!("Invalid HiggsAudioV2 safetensors: {e}"))?;

        let load_f32 = |name: &str| -> std::result::Result<Tensor, String> {
            let view = st
                .tensor(name)
                .map_err(|e| format!("Missing encoder tensor {name}: {e}"))?;
            let vals: Vec<f32> = match view.dtype() {
                StDtype::F32 => view
                    .data()
                    .chunks_exact(4)
                    .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                    .collect(),
                StDtype::F16 => view
                    .data()
                    .chunks_exact(2)
                    .map(|c| half::f16::from_le_bytes([c[0], c[1]]).to_f32())
                    .collect(),
                StDtype::BF16 => view
                    .data()
                    .chunks_exact(2)
                    .map(|c| half::bf16::from_le_bytes([c[0], c[1]]).to_f32())
                    .collect(),
                other => return Err(format!("Unsupported dtype {other:?} for {name}")),
            };
            Tensor::from_vec(vals, view.shape(), device).map_err(|e| e.to_string())
        };
        let load_bias_1d = |name: &str| -> std::result::Result<Tensor, String> {
            let b = load_f32(name)?;
            let c = b.dim(0).map_err(|e| e.to_string())?;
            b.reshape((1, c, 1)).map_err(|e| e.to_string())
        };
        let load_linear = |prefix: &str| -> std::result::Result<Linear, String> {
            let w = load_f32(&format!("{prefix}.weight"))?
                .t()
                .and_then(|t| t.contiguous())
                .map_err(|e| e.to_string())?;
            let b = load_f32(&format!("{prefix}.bias"))?
                .unsqueeze(0)
                .map_err(|e| e.to_string())?;
            Ok(Linear {
                weight_t: w,
                bias: b,
            })
        };
        let load_ln = |prefix: &str| -> std::result::Result<LayerNorm, String> {
            Ok(LayerNorm {
                weight: load_f32(&format!("{prefix}.weight"))?,
                bias: load_f32(&format!("{prefix}.bias"))?,
                eps: 1e-5,
            })
        };

        // 1. Acoustic DAC Encoder
        let ac_conv1 = Conv1d {
            weight: load_f32("acoustic_encoder.conv1.weight")?,
            bias: Some(load_bias_1d("acoustic_encoder.conv1.bias")?),
            stride: 1,
            padding: 3,
            dilation: 1,
            groups: 1,
        };
        let mut ac_blocks = Vec::with_capacity(DOWNSAMPLING_RATIOS.len());
        for (i, &stride) in DOWNSAMPLING_RATIOS.iter().enumerate() {
            let bp = format!("acoustic_encoder.block.{i}");
            let load_res = |u: usize, dilation: usize| -> std::result::Result<DacResUnit, String> {
                let rp = format!("{bp}.res_unit{u}");
                Ok(DacResUnit {
                    snake1: Snake1d {
                        alpha: load_f32(&format!("{rp}.snake1.alpha"))?,
                    },
                    conv1: Conv1d {
                        weight: load_f32(&format!("{rp}.conv1.weight"))?,
                        bias: Some(load_bias_1d(&format!("{rp}.conv1.bias"))?),
                        stride: 1,
                        padding: 3 * dilation,
                        dilation,
                        groups: 1,
                    },
                    snake2: Snake1d {
                        alpha: load_f32(&format!("{rp}.snake2.alpha"))?,
                    },
                    conv2: Conv1d {
                        weight: load_f32(&format!("{rp}.conv2.weight"))?,
                        bias: Some(load_bias_1d(&format!("{rp}.conv2.bias"))?),
                        stride: 1,
                        padding: 0,
                        dilation: 1,
                        groups: 1,
                    },
                })
            };
            ac_blocks.push(DacEncoderBlock {
                res_units: [load_res(1, 1)?, load_res(2, 3)?, load_res(3, 9)?],
                snake1: Snake1d {
                    alpha: load_f32(&format!("{bp}.snake1.alpha"))?,
                },
                conv1: Conv1d {
                    weight: load_f32(&format!("{bp}.conv1.weight"))?,
                    bias: Some(load_bias_1d(&format!("{bp}.conv1.bias"))?),
                    stride,
                    padding: stride.div_ceil(2),
                    dilation: 1,
                    groups: 1,
                },
            });
        }
        let acoustic_encoder = DacEncoder {
            conv1: ac_conv1,
            blocks: ac_blocks,
            snake1: Snake1d {
                alpha: load_f32("acoustic_encoder.snake1.alpha")?,
            },
            conv2: Conv1d {
                weight: load_f32("acoustic_encoder.conv2.weight")?,
                bias: Some(load_bias_1d("acoustic_encoder.conv2.bias")?),
                stride: 1,
                padding: 1,
                dilation: 1,
                groups: 1,
            },
        };

        // 2. HuBERT Semantic Model
        let feat_conv0 = Conv1d {
            weight: load_f32("semantic_model.feature_extractor.conv_layers.0.conv.weight")?,
            bias: None,
            stride: 5,
            padding: 0,
            dilation: 1,
            groups: 1,
        };
        let feat_gn0_w = load_bias_1d("semantic_model.feature_extractor.conv_layers.0.layer_norm.weight")?;
        let feat_gn0_b = load_bias_1d("semantic_model.feature_extractor.conv_layers.0.layer_norm.bias")?;
        let mut feat_convs = Vec::with_capacity(6);
        for l in 1..7 {
            feat_convs.push(Conv1d {
                weight: load_f32(&format!(
                    "semantic_model.feature_extractor.conv_layers.{l}.conv.weight"
                ))?,
                bias: None,
                stride: 2,
                padding: 0,
                dilation: 1,
                groups: 1,
            });
        }
        let proj_ln = load_ln("semantic_model.feature_projection.layer_norm")?;
        let proj_linear = load_linear("semantic_model.feature_projection.projection")?;

        // Reconstruct weight-normalized pos_conv_embed kernel: w = g * v / ||v||_{dim=(0,1)}
        let g = load_f32("semantic_model.encoder.pos_conv_embed.conv.parametrizations.weight.original0")?; // [1, 1, 128]
        let v = load_f32("semantic_model.encoder.pos_conv_embed.conv.parametrizations.weight.original1")?; // [768, 48, 128]
        let v_norm = v
            .sqr()
            .and_then(|t| t.sum_keepdim(0))
            .and_then(|t| t.sum_keepdim(1))
            .and_then(|t| t + 1e-12)
            .and_then(|t| t.sqrt())
            .map_err(|e| e.to_string())?;
        let pos_w = v
            .broadcast_mul(&g)
            .and_then(|t| t.broadcast_div(&v_norm))
            .map_err(|e| e.to_string())?;
        let pos_conv = Conv1d {
            weight: pos_w,
            bias: Some(load_bias_1d("semantic_model.encoder.pos_conv_embed.conv.bias")?),
            stride: 1,
            padding: 64,
            dilation: 1,
            groups: 16,
        };
        let enc_ln = load_ln("semantic_model.encoder.layer_norm")?;
        let mut layers = Vec::with_capacity(12);
        for l in 0..12 {
            let lp = format!("semantic_model.encoder.layers.{l}");
            layers.push(HubertEncoderLayer {
                q_proj: load_linear(&format!("{lp}.attention.q_proj"))?,
                k_proj: load_linear(&format!("{lp}.attention.k_proj"))?,
                v_proj: load_linear(&format!("{lp}.attention.v_proj"))?,
                out_proj: load_linear(&format!("{lp}.attention.out_proj"))?,
                layer_norm: load_ln(&format!("{lp}.layer_norm"))?,
                intermediate_dense: load_linear(&format!("{lp}.feed_forward.intermediate_dense"))?,
                output_dense: load_linear(&format!("{lp}.feed_forward.output_dense"))?,
                final_layer_norm: load_ln(&format!("{lp}.final_layer_norm"))?,
            });
        }
        let semantic_model = HubertSemanticModel {
            feat_conv0,
            feat_gn0_w,
            feat_gn0_b,
            feat_convs,
            proj_ln,
            proj_linear,
            pos_conv,
            enc_ln,
            layers,
        };

        // 3. SemanticEncoder
        let load_sem_block = |bi: usize| -> std::result::Result<SemanticEncoderBlock, String> {
            let bp = format!("encoder_semantic.conv_blocks.{bi}");
            let load_ru = |ri: usize| -> std::result::Result<SemanticResUnit, String> {
                Ok(SemanticResUnit {
                    conv1: Conv1d {
                        weight: load_f32(&format!("{bp}.res_units.{ri}.conv1.weight"))?,
                        bias: None,
                        stride: 1,
                        padding: 1,
                        dilation: 1,
                        groups: 1,
                    },
                    conv2: Conv1d {
                        weight: load_f32(&format!("{bp}.res_units.{ri}.conv2.weight"))?,
                        bias: None,
                        stride: 1,
                        padding: 0,
                        dilation: 1,
                        groups: 1,
                    },
                })
            };
            Ok(SemanticEncoderBlock {
                res_units: [load_ru(0)?, load_ru(1)?],
                conv: Conv1d {
                    weight: load_f32(&format!("{bp}.conv.weight"))?,
                    bias: Some(load_bias_1d(&format!("{bp}.conv.bias"))?),
                    stride: 1,
                    padding: 1,
                    dilation: 1,
                    groups: 1,
                },
            })
        };
        let encoder_semantic = SemanticEncoder {
            conv: Conv1d {
                weight: load_f32("encoder_semantic.conv.weight")?,
                bias: None,
                stride: 1,
                padding: 1,
                dilation: 1,
                groups: 1,
            },
            blocks: [load_sem_block(0)?, load_sem_block(1)?],
        };

        // 4. Fusion + RVQ quantizers
        let fc = load_linear("fc")?;
        let mut quantizers = Vec::with_capacity(CODEBOOKS);
        for i in 0..CODEBOOKS {
            let p = format!("quantizer.quantizers.{i}");
            let embed = load_f32(&format!("{p}.codebook.embed"))?;
            let embed_sq = embed
                .sqr()
                .and_then(|t| t.sum(1))
                .and_then(|t| t.unsqueeze(0))
                .map_err(|e| e.to_string())?;
            quantizers.push(RvqEncoderLayer {
                project_in: load_linear(&format!("{p}.project_in"))?,
                project_out: load_linear(&format!("{p}.project_out"))?,
                embed,
                embed_sq,
            });
        }

        Ok(Self {
            acoustic_encoder,
            semantic_model,
            encoder_semantic,
            fc,
            quantizers,
            device: device.clone(),
        })
    }

    /// Encode a reference `.wav` recording + its exact transcript into a reusable `Voice`.
    pub fn encode_wav_file(
        &self,
        wav_path: &Path,
        transcript: &str,
        language: &str,
    ) -> std::result::Result<Voice, String> {
        let lang = text::check_language(language)?;
        let (raw_samples, src_sr) = read_wav_mono_f32(wav_path)?;
        if raw_samples.is_empty() {
            return Err(format!("Reference WAV {} is empty", wav_path.display()));
        }
        let wav_24k = resample_kaiser(&raw_samples, src_sr, ACOUSTIC_SR);
        let mut wav_24k = raise_quiet(wav_24k);
        // Pad to whole codec frames (multiple of hop_length = 960)
        let rem = wav_24k.len() % HOP_LENGTH;
        if rem != 0 {
            wav_24k.resize(wav_24k.len() + (HOP_LENGTH - rem), 0.0);
        }
        let speech_secs = speech_length_secs(&wav_24k, ACOUSTIC_SR);

        // Semantic branch: resample 24k -> 16k and zero-pad (160, 160)
        let wav_16k = resample_kaiser(&wav_24k, ACOUSTIC_SR, SEMANTIC_SR);
        let mut wav_16k_pad = Vec::with_capacity(wav_16k.len() + 320);
        wav_16k_pad.resize(160, 0.0);
        wav_16k_pad.extend_from_slice(&wav_16k);
        wav_16k_pad.resize(wav_16k_pad.len() + 160, 0.0);

        let sem_in = Tensor::from_vec(wav_16k_pad, (1, 1, wav_16k.len() + 320), &self.device)
            .map_err(|e| e.to_string())?;
        let e_sem_in = self
            .semantic_model
            .forward(&sem_in)
            .map_err(|e| format!("HuBERT semantic encode failed: {e}"))?;
        let e_semantic = self
            .encoder_semantic
            .forward(&e_sem_in.transpose(1, 2).and_then(|t| t.contiguous()).map_err(|e| e.to_string())?)
            .map_err(|e| format!("SemanticEncoder failed: {e}"))?;
        let sem_frames = e_semantic.dim(2).map_err(|e| e.to_string())?;

        // Acoustic branch: run unpadded first; if output length != sem_frames, re-run with (480, 480) padding
        let ac_raw = Tensor::from_vec(wav_24k.clone(), (1, 1, wav_24k.len()), &self.device)
            .map_err(|e| e.to_string())?;
        let mut e_acoustic = self
            .acoustic_encoder
            .forward(&ac_raw)
            .map_err(|e| format!("AcousticEncoder failed: {e}"))?;
        if e_acoustic.dim(2).map_err(|e| e.to_string())? != sem_frames {
            let pad = HOP_LENGTH / 2;
            let mut wav_24k_pad = Vec::with_capacity(wav_24k.len() + pad * 2);
            wav_24k_pad.resize(pad, 0.0);
            wav_24k_pad.extend_from_slice(&wav_24k);
            wav_24k_pad.resize(wav_24k_pad.len() + pad, 0.0);
            let ac_in = Tensor::from_vec(wav_24k_pad, (1, 1, wav_24k.len() + pad * 2), &self.device)
                .map_err(|e| e.to_string())?;
            e_acoustic = self
                .acoustic_encoder
                .forward(&ac_in)
                .map_err(|e| format!("AcousticEncoder (padded) failed: {e}"))?;
        }

        let ac_frames = e_acoustic.dim(2).map_err(|e| e.to_string())?;
        let frames = ac_frames.min(sem_frames);
        let e_ac = e_acoustic.narrow(2, 0, frames).map_err(|e| e.to_string())?;
        let e_sem = e_semantic.narrow(2, 0, frames).map_err(|e| e.to_string())?;

        let fused = Tensor::cat(&[&e_ac, &e_sem], 1)
            .and_then(|t| t.squeeze(0))
            .and_then(|t| t.t())
            .and_then(|t| t.contiguous())
            .map_err(|e| e.to_string())?; // [F, 1024]
        let mut residual = self
            .fc
            .forward(&fused)
            .map_err(|e| format!("Fusion fc failed: {e}"))?;

        let mut codes = Vec::with_capacity(CODEBOOKS * frames);
        for (cb, q) in self.quantizers.iter().enumerate() {
            let (idx, next_res) = q
                .encode_step(&residual)
                .map_err(|e| format!("RVQ encode cb {cb} failed: {e}"))?;
            codes.extend_from_slice(&idx);
            residual = next_res;
        }

        let speech_frames = ((speech_secs * FRAME_RATE as f32).round() as usize).clamp(1, frames);
        let prepared_transcript = text::prepare_text(transcript, lang);
        let name = wav_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("custom")
            .to_owned();
        Ok(Voice {
            name,
            language: lang.to_owned(),
            transcript: prepared_transcript,
            codes,
            frames,
            speech_frames,
        })
    }
}
