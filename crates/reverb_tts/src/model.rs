//! Reverb-TTS Qwen3 backbone, factorized text embeddings, audio codebook
//! embeddings/heads, and INT4/INT8/FP16/BF16 safetensors weight loader.

use candle_core::{DType, Device, Result, Tensor};
use candle_nn::ops::{silu, softmax_last_dim};
use half::{bf16, f16};
use safetensors::{Dtype as StDtype, SafeTensors};
use serde::Deserialize;
use std::path::Path;

pub const CODEBOOKS: usize = 8;
pub const CODES: usize = 1025;
pub const MASK: u32 = 1024;
const QUANTIZED_PREFIX: &str = "llm.layers.";

#[derive(Debug, Deserialize)]
struct ReverbConfigFile {
    llm_config: LlmConfig,
    #[serde(default)]
    quantization: Option<QuantConfig>,
}

#[derive(Debug, Deserialize)]
struct RopeParams {
    #[serde(default = "default_rope_theta")]
    rope_theta: f64,
}
fn default_rope_theta() -> f64 {
    1_000_000.0
}

#[derive(Debug, Deserialize)]
struct LlmConfig {
    hidden_size: usize,
    intermediate_size: usize,
    num_hidden_layers: usize,
    num_attention_heads: usize,
    num_key_value_heads: usize,
    #[serde(default = "default_head_dim")]
    head_dim: usize,
    #[serde(default = "default_rms_eps")]
    rms_norm_eps: f64,
    #[serde(default)]
    rope_parameters: Option<RopeParams>,
    #[serde(default)]
    rope_theta: Option<f64>,
}
fn default_head_dim() -> usize {
    128
}
fn default_rms_eps() -> f64 {
    1e-6
}

#[derive(Debug, Deserialize)]
struct QuantConfig {
    bits: u8,
    #[serde(default)]
    group_size: Option<usize>,
    #[serde(default)]
    rotation: Option<serde_json::Value>,
    #[serde(default)]
    activations: Option<serde_json::Value>,
}

struct RmsNorm {
    weight: Tensor, // [dim] in F32
    eps: f64,
}

impl RmsNorm {
    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        let orig_dtype = x.dtype();
        let xf = x.to_dtype(DType::F32)?;
        let mean_sq = xf.sqr()?.mean_keepdim(candle_core::D::Minus1)?;
        let normed = xf.broadcast_div(&(mean_sq + self.eps)?.sqrt()?)?;
        let scaled = normed.broadcast_mul(&self.weight)?;
        scaled.to_dtype(orig_dtype)
    }
}

struct Linear {
    weight_t: Tensor, // [in_dim, out_dim] transposed once at load time for fast matmul
}

impl Linear {
    fn from_weight(w: Tensor) -> Result<Self> {
        // w is [out_dim, in_dim]
        let weight_t = w.t()?.contiguous()?;
        Ok(Self { weight_t })
    }

    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        // Support 2D [M, K] or 3D [B, M, K]
        match x.dims() {
            [b, m, k] => {
                let flat = x.reshape((b * m, *k))?;
                let out = flat.matmul(&self.weight_t)?;
                let out_dim = self.weight_t.dim(1)?;
                out.reshape((*b, *m, out_dim))
            }
            _ => x.matmul(&self.weight_t),
        }
    }
}

enum TextEmbed {
    Factorized {
        table: Tensor,   // [vocab, rank]
        project: Linear, // rank -> hidden
    },
    Plain(Tensor), // [vocab, hidden]
}

impl TextEmbed {
    fn forward(&self, ids: &Tensor) -> Result<Tensor> {
        match self {
            Self::Factorized { table, project } => {
                let rows = table.index_select(ids, 0)?;
                project.forward(&rows)
            }
            Self::Plain(table) => table.index_select(ids, 0),
        }
    }
}

struct Qwen3Attention {
    q_proj: Linear,
    k_proj: Linear,
    v_proj: Linear,
    o_proj: Linear,
    q_norm: RmsNorm,
    k_norm: RmsNorm,
    num_heads: usize,
    num_kv_heads: usize,
    head_dim: usize,
    scale: f64,
}

impl Qwen3Attention {
    fn apply_rope(x: &Tensor, cos: &Tensor, sin: &Tensor) -> Result<Tensor> {
        // x: [1, heads, seq, head_dim], cos/sin: [1, 1, seq, head_dim/2] in F32
        let orig_dtype = x.dtype();
        let xf = x.to_dtype(DType::F32)?;
        let half = xf.dim(3)? / 2;
        let x1 = xf.narrow(3, 0, half)?;
        let x2 = xf.narrow(3, half, half)?;
        let y1 = (x1.broadcast_mul(cos)? - x2.broadcast_mul(sin)?)?;
        let y2 = (x2.broadcast_mul(cos)? + x1.broadcast_mul(sin)?)?;
        Tensor::cat(&[&y1, &y2], 3)?.to_dtype(orig_dtype)
    }

    fn repeat_kv(x: &Tensor, n_rep: usize) -> Result<Tensor> {
        if n_rep == 1 {
            return Ok(x.clone());
        }
        let (b, kv_heads, seq, head_dim) = x.dims4()?;
        x.unsqueeze(2)?
            .expand((b, kv_heads, n_rep, seq, head_dim))?
            .reshape((b, kv_heads * n_rep, seq, head_dim))
    }

    /// Prefill prefix: returns `(output, (k_rope, v))` where `k_rope, v` are `[1, kv_heads, P, head_dim]`.
    fn prefill(&self, x: &Tensor, cos: &Tensor, sin: &Tensor) -> Result<(Tensor, (Tensor, Tensor))> {
        let (b, seq, _) = x.dims3()?;
        let q = self
            .q_proj
            .forward(x)?
            .reshape((b, seq, self.num_heads, self.head_dim))?;
        let q = self.q_norm.forward(&q)?.transpose(1, 2)?.contiguous()?;
        let k = self
            .k_proj
            .forward(x)?
            .reshape((b, seq, self.num_kv_heads, self.head_dim))?;
        let k = self.k_norm.forward(&k)?.transpose(1, 2)?.contiguous()?;
        let v = self
            .v_proj
            .forward(x)?
            .reshape((b, seq, self.num_kv_heads, self.head_dim))?
            .transpose(1, 2)?
            .contiguous()?;

        let q = Self::apply_rope(&q, cos, sin)?;
        let k = Self::apply_rope(&k, cos, sin)?;

        let out = self.attend(&q, &k, &v, None)?;
        Ok((out, (k, v)))
    }

    /// Target diffusion pass attending over `[prefix_kv, target_kv]` without mutating `prefix_kv`.
    fn forward_target(
        &self,
        x: &Tensor,
        cos: &Tensor,
        sin: &Tensor,
        prefix_kv: &(Tensor, Tensor),
        attn_bias: Option<&Tensor>,
    ) -> Result<Tensor> {
        let (b, seq, _) = x.dims3()?;
        let q = self
            .q_proj
            .forward(x)?
            .reshape((b, seq, self.num_heads, self.head_dim))?;
        let q = self.q_norm.forward(&q)?.transpose(1, 2)?.contiguous()?;
        let k = self
            .k_proj
            .forward(x)?
            .reshape((b, seq, self.num_kv_heads, self.head_dim))?;
        let k = self.k_norm.forward(&k)?.transpose(1, 2)?.contiguous()?;
        let v = self
            .v_proj
            .forward(x)?
            .reshape((b, seq, self.num_kv_heads, self.head_dim))?
            .transpose(1, 2)?
            .contiguous()?;

        let q = Self::apply_rope(&q, cos, sin)?;
        let k = Self::apply_rope(&k, cos, sin)?;

        let k_all = Tensor::cat(&[&prefix_kv.0, &k], 2)?;
        let v_all = Tensor::cat(&[&prefix_kv.1, &v], 2)?;
        self.attend(&q, &k_all, &v_all, attn_bias)
    }

    fn attend(
        &self,
        q: &Tensor,
        k: &Tensor,
        v: &Tensor,
        attn_bias: Option<&Tensor>,
    ) -> Result<Tensor> {
        let (b, _, q_len, _) = q.dims4()?;
        let dtype = q.dtype();
        let n_rep = self.num_heads / self.num_kv_heads;
        let k_exp = Self::repeat_kv(k, n_rep)?;
        let v_exp = Self::repeat_kv(v, n_rep)?;

        let kt = k_exp.transpose(2, 3)?.contiguous()?;
        let mut scores = (q.contiguous()?.matmul(&kt)?.to_dtype(DType::F32)? * self.scale)?;
        if let Some(bias) = attn_bias {
            scores = scores.broadcast_add(bias)?;
        }
        let probs = softmax_last_dim(&scores)?.to_dtype(dtype)?;
        let ctx = probs
            .matmul(&v_exp.contiguous()?)?
            .transpose(1, 2)?
            .contiguous()?
            .reshape((b, q_len, self.num_heads * self.head_dim))?;
        self.o_proj.forward(&ctx)
    }
}

struct Qwen3Mlp {
    gate_proj: Linear,
    up_proj: Linear,
    down_proj: Linear,
}

impl Qwen3Mlp {
    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        let gate = silu(&self.gate_proj.forward(x)?)?;
        let up = self.up_proj.forward(x)?;
        self.down_proj.forward(&(gate * up)?)
    }
}

struct Qwen3Layer {
    input_layernorm: RmsNorm,
    self_attn: Qwen3Attention,
    post_attention_layernorm: RmsNorm,
    mlp: Qwen3Mlp,
}

impl Qwen3Layer {
    fn prefill(&self, x: &Tensor, cos: &Tensor, sin: &Tensor) -> Result<(Tensor, (Tensor, Tensor))> {
        let h = self.input_layernorm.forward(x)?;
        let (attn_out, kv) = self.self_attn.prefill(&h, cos, sin)?;
        let x = (x + attn_out)?;
        let mlp_out = self.mlp.forward(&self.post_attention_layernorm.forward(&x)?)?;
        Ok(((x + mlp_out)?, kv))
    }

    fn forward_target(
        &self,
        x: &Tensor,
        cos: &Tensor,
        sin: &Tensor,
        prefix_kv: &(Tensor, Tensor),
        attn_bias: Option<&Tensor>,
    ) -> Result<Tensor> {
        let h = self.input_layernorm.forward(x)?;
        let attn_out = self
            .self_attn
            .forward_target(&h, cos, sin, prefix_kv, attn_bias)?;
        let x = (x + attn_out)?;
        let mlp_out = self.mlp.forward(&self.post_attention_layernorm.forward(&x)?)?;
        x + mlp_out
    }
}

pub struct PrefixCache {
    layers: Vec<(Tensor, Tensor)>,
    pub prefix_len: usize,
    pub ref_len: usize,
}

pub struct ReverbNetwork {
    embed_tokens: TextEmbed,
    audio_embeddings: Tensor, // [8 * 1025, hidden]
    audio_heads: Tensor,      // [8 * 1025, hidden] (centered per codebook)
    layers: Vec<Qwen3Layer>,
    norm: RmsNorm,
    rope_inv_freq: Vec<f32>,
    pub device: Device,
    pub dtype: DType,
}

impl ReverbNetwork {
    pub fn load(weights_dir: &Path, device: &Device, dtype: DType) -> std::result::Result<Self, String> {
        let cfg_raw = std::fs::read_to_string(weights_dir.join("config.json"))
            .map_err(|e| format!("Cannot read Reverb config.json: {e}"))?;
        let cfg: ReverbConfigFile = serde_json::from_str(&cfg_raw)
            .map_err(|e| format!("Invalid Reverb config.json: {e}"))?;
        if let Some(q) = &cfg.quantization {
            if q.rotation.is_some() {
                return Err("Rotated Reverb checkpoints are not supported".into());
            }
            if q.activations.is_some() {
                return Err("Activation-quantized Reverb checkpoints are not supported".into());
            }
        }
        let st_bytes = std::fs::read(weights_dir.join("model.safetensors"))
            .map_err(|e| format!("Cannot read Reverb model.safetensors: {e}"))?;
        let st = SafeTensors::deserialize(&st_bytes)
            .map_err(|e| format!("Invalid Reverb model.safetensors: {e}"))?;

        let load_tensor = |name: &str, target_dtype: DType| -> std::result::Result<Tensor, String> {
            load_and_dequantize(&st, name, cfg.quantization.as_ref(), device, target_dtype)
        };

        let llm = &cfg.llm_config;
        let embed_tokens = if st.tensor("llm.embed_tokens.table.weight").is_ok() {
            let table = load_tensor("llm.embed_tokens.table.weight", dtype)?;
            let proj_w = load_tensor("llm.embed_tokens.project.weight", dtype)?;
            TextEmbed::Factorized {
                table,
                project: Linear::from_weight(proj_w).map_err(|e| e.to_string())?,
            }
        } else {
            TextEmbed::Plain(load_tensor("llm.embed_tokens.weight", dtype)?)
        };

        let audio_embeddings = load_tensor("audio_embeddings.weight", dtype)?;
        // Center each codebook's head rows in F32 (matches reverb_tts/model.py:63-64)
        let heads_f32 = load_tensor("audio_heads.weight", DType::F32)?;
        let heads_3d = heads_f32
            .reshape((CODEBOOKS, CODES, llm.hidden_size))
            .map_err(|e| e.to_string())?;
        let mean = heads_3d.mean_keepdim(1).map_err(|e| e.to_string())?;
        let audio_heads = heads_3d
            .broadcast_sub(&mean)
            .map_err(|e| e.to_string())?
            .reshape((CODEBOOKS * CODES, llm.hidden_size))
            .map_err(|e| e.to_string())?
            .to_dtype(dtype)
            .map_err(|e| e.to_string())?;

        let mut layers = Vec::with_capacity(llm.num_hidden_layers);
        for i in 0..llm.num_hidden_layers {
            let p = format!("llm.layers.{i}");
            let input_layernorm = RmsNorm {
                weight: load_tensor(&format!("{p}.input_layernorm.weight"), DType::F32)?,
                eps: llm.rms_norm_eps,
            };
            let post_attention_layernorm = RmsNorm {
                weight: load_tensor(&format!("{p}.post_attention_layernorm.weight"), DType::F32)?,
                eps: llm.rms_norm_eps,
            };
            let self_attn = Qwen3Attention {
                q_proj: Linear::from_weight(load_tensor(&format!("{p}.self_attn.q_proj.weight"), dtype)?)
                    .map_err(|e| e.to_string())?,
                k_proj: Linear::from_weight(load_tensor(&format!("{p}.self_attn.k_proj.weight"), dtype)?)
                    .map_err(|e| e.to_string())?,
                v_proj: Linear::from_weight(load_tensor(&format!("{p}.self_attn.v_proj.weight"), dtype)?)
                    .map_err(|e| e.to_string())?,
                o_proj: Linear::from_weight(load_tensor(&format!("{p}.self_attn.o_proj.weight"), dtype)?)
                    .map_err(|e| e.to_string())?,
                q_norm: RmsNorm {
                    weight: load_tensor(&format!("{p}.self_attn.q_norm.weight"), DType::F32)?,
                    eps: llm.rms_norm_eps,
                },
                k_norm: RmsNorm {
                    weight: load_tensor(&format!("{p}.self_attn.k_norm.weight"), DType::F32)?,
                    eps: llm.rms_norm_eps,
                },
                num_heads: llm.num_attention_heads,
                num_kv_heads: llm.num_key_value_heads,
                head_dim: llm.head_dim,
                scale: 1.0 / (llm.head_dim as f64).sqrt(),
            };
            let mlp = Qwen3Mlp {
                gate_proj: Linear::from_weight(load_tensor(&format!("{p}.mlp.gate_proj.weight"), dtype)?)
                    .map_err(|e| e.to_string())?,
                up_proj: Linear::from_weight(load_tensor(&format!("{p}.mlp.up_proj.weight"), dtype)?)
                    .map_err(|e| e.to_string())?,
                down_proj: Linear::from_weight(load_tensor(&format!("{p}.mlp.down_proj.weight"), dtype)?)
                    .map_err(|e| e.to_string())?,
            };
            layers.push(Qwen3Layer {
                input_layernorm,
                self_attn,
                post_attention_layernorm,
                mlp,
            });
        }

        let norm = RmsNorm {
            weight: load_tensor("llm.norm.weight", DType::F32)?,
            eps: llm.rms_norm_eps,
        };

        let rope_theta = llm
            .rope_parameters
            .as_ref()
            .map(|r| r.rope_theta)
            .or(llm.rope_theta)
            .unwrap_or(1_000_000.0);
        let half_dim = llm.head_dim / 2;
        let rope_inv_freq: Vec<f32> = (0..half_dim)
            .map(|i| (1.0 / rope_theta.powf((2 * i) as f64 / llm.head_dim as f64)) as f32)
            .collect();

        let _ = llm.intermediate_size;
        Ok(Self {
            embed_tokens,
            audio_embeddings,
            audio_heads,
            layers,
            norm,
            rope_inv_freq,
            device: device.clone(),
            dtype,
        })
    }

    fn rope_cos_sin(&self, pos_start: usize, seq_len: usize) -> Result<(Tensor, Tensor)> {
        let half = self.rope_inv_freq.len();
        let mut cos_v = Vec::with_capacity(seq_len * half);
        let mut sin_v = Vec::with_capacity(seq_len * half);
        for p in pos_start..(pos_start + seq_len) {
            let pf = p as f32;
            for &freq in &self.rope_inv_freq {
                let angle = pf * freq;
                cos_v.push(angle.cos());
                sin_v.push(angle.sin());
            }
        }
        let cos = Tensor::from_vec(cos_v, (1, 1, seq_len, half), &self.device)?;
        let sin = Tensor::from_vec(sin_v, (1, 1, seq_len, half), &self.device)?;
        Ok((cos, sin))
    }

    /// Embed text prompt token IDs -> `[1, len, hidden]`.
    pub fn embed_text(&self, ids: &[u32]) -> Result<Tensor> {
        let idx = Tensor::from_slice(ids, ids.len(), &self.device)?;
        self.embed_tokens.forward(&idx)?.unsqueeze(0)
    }

    /// Embed 8-codebook audio frame tokens (`[8, frames]` row-major) by summing across codebooks -> `[1, frames, hidden]`.
    pub fn embed_audio(&self, codes: &[u32], frames: usize) -> Result<Tensor> {
        let mut offset_ids = Vec::with_capacity(CODEBOOKS * frames);
        for cb in 0..CODEBOOKS {
            let base = (cb * CODES) as u32;
            let row = &codes[cb * frames..(cb + 1) * frames];
            for &c in row {
                offset_ids.push(base + c);
            }
        }
        let idx = Tensor::from_vec(offset_ids, CODEBOOKS * frames, &self.device)?;
        let emb = self
            .audio_embeddings
            .index_select(&idx, 0)?
            .reshape((CODEBOOKS, frames, self.audio_embeddings.dim(1)?))?;
        emb.sum(0)?.unsqueeze(0)
    }

    /// Run prefix `[text_embeds, ref_audio_embeds]` once and capture per-layer KV cache.
    pub fn prefill_prefix(&self, text_ids: &[u32], ref_codes: &[u32], ref_frames: usize) -> Result<PrefixCache> {
        let t_emb = self.embed_text(text_ids)?;
        let r_emb = self.embed_audio(ref_codes, ref_frames)?;
        let mut hidden = Tensor::cat(&[&t_emb, &r_emb], 1)?;
        let prefix_len = hidden.dim(1)?;
        let (cos, sin) = self.rope_cos_sin(0, prefix_len)?;
        let mut layer_kvs = Vec::with_capacity(self.layers.len());
        for layer in &self.layers {
            let (next_h, kv) = layer.prefill(&hidden, &cos, &sin)?;
            hidden = next_h;
            layer_kvs.push(kv);
        }
        Ok(PrefixCache {
            layers: layer_kvs,
            prefix_len,
            ref_len: ref_frames,
        })
    }

    /// Run one diffusion pass over target tokens `[8, F]` and compute F32 log-probs `[num_cb, F, 1024]`.
    pub fn target_log_probs(
        &self,
        tokens: &[u32],
        frames: usize,
        cache: &PrefixCache,
        cb_start: usize,
        cb_end: usize,
        polish_bias: Option<f32>,
    ) -> Result<Tensor> {
        let mut hidden = self.embed_audio(tokens, frames)?;
        let (cos, sin) = self.rope_cos_sin(cache.prefix_len, frames)?;

        let attn_bias = if let Some(bias_val) = polish_bias {
            let total_kv = cache.prefix_len + frames;
            let ref_start = cache.prefix_len.saturating_sub(cache.ref_len);
            let mut bias = vec![0.0_f32; total_kv];
            for slot in &mut bias[ref_start..cache.prefix_len] {
                *slot = bias_val;
            }
            Some(Tensor::from_vec(bias, (1, 1, 1, total_kv), &self.device)?)
        } else {
            None
        };

        for (layer, kv) in self.layers.iter().zip(&cache.layers) {
            hidden = layer.forward_target(&hidden, &cos, &sin, kv, attn_bias.as_ref())?;
        }
        let hidden = self.norm.forward(&hidden)?.squeeze(0)?; // [F, hidden]

        let num_cb = cb_end - cb_start;
        let head_slice = self
            .audio_heads
            .narrow(0, cb_start * CODES, num_cb * CODES)?
            .t()?
            .contiguous()?; // [hidden, num_cb * CODES]
        let logits = hidden
            .matmul(&head_slice)?
            .to_dtype(DType::F32)?
            .reshape((frames, num_cb, CODES))?
            .transpose(0, 1)?
            .contiguous()?; // [num_cb, F, 1025]
        let log_probs = candle_nn::ops::log_softmax(&logits, 2)?;
        log_probs.narrow(2, 0, MASK as usize)?.contiguous() // [num_cb, F, 1024]
    }
}

fn view_to_f32_vec(view: &safetensors::tensor::TensorView<'_>) -> std::result::Result<Vec<f32>, String> {
    let data = view.data();
    match view.dtype() {
        StDtype::F32 => Ok(data
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect()),
        StDtype::F16 => Ok(data
            .chunks_exact(2)
            .map(|c| f16::from_le_bytes([c[0], c[1]]).to_f32())
            .collect()),
        StDtype::BF16 => Ok(data
            .chunks_exact(2)
            .map(|c| bf16::from_le_bytes([c[0], c[1]]).to_f32())
            .collect()),
        other => Err(format!("Unsupported float tensor dtype {other:?}")),
    }
}

fn upload_f32_as(
    vals: Vec<f32>,
    shape: &[usize],
    device: &Device,
    target_dtype: DType,
) -> std::result::Result<Tensor, String> {
    match target_dtype {
        DType::F16 => {
            let halfs: Vec<f16> = vals.into_iter().map(f16::from_f32).collect();
            Tensor::from_vec(halfs, shape, device).map_err(|e| e.to_string())
        }
        DType::BF16 => {
            let halfs: Vec<bf16> = vals.into_iter().map(bf16::from_f32).collect();
            Tensor::from_vec(halfs, shape, device).map_err(|e| e.to_string())
        }
        _ => Tensor::from_vec(vals, shape, device)
            .and_then(|t| t.to_dtype(target_dtype))
            .map_err(|e| e.to_string()),
    }
}

fn load_and_dequantize(
    st: &SafeTensors<'_>,
    name: &str,
    quant: Option<&QuantConfig>,
    device: &Device,
    target_dtype: DType,
) -> std::result::Result<Tensor, String> {
    let view = st
        .tensor(name)
        .map_err(|e| format!("Missing tensor {name}: {e}"))?;
    let scale_name = format!("{name}_scale");
    let has_scale = st.tensor(&scale_name).is_ok();

    if has_scale {
        let scale_view = st.tensor(&scale_name).map_err(|e| e.to_string())?;
        let scales = view_to_f32_vec(&scale_view)?;
        let shape = view.shape();
        let bits = quant.map_or(8, |q| q.bits);
        let group_size = quant.and_then(|q| q.group_size);
        let is_proj = name.starts_with(QUANTIZED_PREFIX);

        if is_proj && bits == 4 && view.dtype() == StDtype::U8 {
            if shape.len() != 2 {
                return Err(format!("Expected 2D int4 tensor for {name}, got {shape:?}"));
            }
            let rows = shape[0];
            let packed_cols = shape[1];
            let cols = packed_cols * 2;
            let raw = view.data();
            let gs = group_size.unwrap_or(cols);
            let groups_per_row = cols / gs;
            let mut out = vec![0.0_f32; rows * cols];
            for r in 0..rows {
                let raw_row = &raw[r * packed_cols..(r + 1) * packed_cols];
                let scale_row = &scales[r * groups_per_row..(r + 1) * groups_per_row];
                let out_row = &mut out[r * cols..(r + 1) * cols];
                for (j, &byte) in raw_row.iter().enumerate() {
                    let c0 = j * 2;
                    let c1 = c0 + 1;
                    let q0 = ((byte & 0x0F) as i32 - 8) as f32;
                    let q1 = ((byte >> 4) as i32 - 8) as f32;
                    out_row[c0] = q0 * scale_row[c0 / gs];
                    out_row[c1] = q1 * scale_row[c1 / gs];
                }
            }
            return upload_f32_as(out, &[rows, cols], device, target_dtype)
                .map_err(|e| format!("Failed uploading int4 tensor {name}: {e}"));
        }

        if view.dtype() == StDtype::I8 {
            if shape.len() != 2 {
                return Err(format!("Expected 2D int8 tensor for {name}, got {shape:?}"));
            }
            let rows = shape[0];
            let cols = shape[1];
            let raw = view.data();
            let groups_per_row = scales.len() / rows.max(1);
            let gs = (cols / groups_per_row.max(1)).max(1);
            let mut out = vec![0.0_f32; rows * cols];
            for r in 0..rows {
                let raw_row = &raw[r * cols..(r + 1) * cols];
                let scale_row = &scales[r * groups_per_row..(r + 1) * groups_per_row];
                let out_row = &mut out[r * cols..(r + 1) * cols];
                for c in 0..cols {
                    let q = (raw_row[c] as i8) as f32;
                    out_row[c] = q * scale_row[(c / gs).min(groups_per_row - 1)];
                }
            }
            return upload_f32_as(out, &[rows, cols], device, target_dtype)
                .map_err(|e| format!("Failed uploading int8 tensor {name}: {e}"));
        }
    }

    let vals = view_to_f32_vec(&view)?;
    upload_f32_as(vals, view.shape(), device, target_dtype)
        .map_err(|e| format!("Failed uploading tensor {name}: {e}"))
}
