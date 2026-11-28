//! 10-pass masked-diffusion sampler for Reverb-TTS (exact port of `reverb_tts/sampler.py`).
//! 8 main passes commit codebooks 0–1 (content & timing), followed by 2 polish passes
//! with reference attention bias for codebooks 2–7.

use crate::model::{CODEBOOKS, MASK, ReverbNetwork};

#[derive(Clone, Copy, Debug)]
pub struct SamplerConfig {
    pub steps: usize,
    pub remasking: bool,
    pub remask_ratio: f32,
    pub t_shift: f32,
    pub codebook_penalty: f32,
    pub position_temperature: f32,
    pub polish_bias: f32,
    pub polish_guidance: f32,
}

impl Default for SamplerConfig {
    fn default() -> Self {
        Self {
            steps: 8,
            remasking: true,
            remask_ratio: 0.25,
            t_shift: 0.1,
            codebook_penalty: 5.0,
            position_temperature: 5.0,
            polish_bias: 1.0,
            polish_guidance: 0.5,
        }
    }
}

/// Deterministic PRNG (SplitMix64) so identical seeds produce reproducible speech.
struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_add(0x9E37_79B9_7F4A_7C15))
    }
    fn next_f32(&mut self) -> f32 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        // Uniform in (0, 1)
        ((z >> 40) as f32 + 0.5) / ((1u64 << 24) as f32)
    }
}

/// Generate `[8, frames]` (row-major) discrete codec tokens in `0..1024`.
pub fn sample_take(
    network: &ReverbNetwork,
    prompt_ids: &[u32],
    ref_codes: &[u32],
    ref_frames: usize,
    target_frames: usize,
    seed: u64,
    cfg: &SamplerConfig,
) -> Result<Vec<u32>, String> {
    let f = target_frames.max(1);
    let cache = network
        .prefill_prefix(prompt_ids, ref_codes, ref_frames)
        .map_err(|e| format!("Prefix prefill failed: {e}"))?;

    let mut tokens = vec![MASK; CODEBOOKS * f];
    let mut rng = Rng::new(seed);

    // Schedule t in [0, 1] warped by t_shift
    let mut schedule = Vec::with_capacity(cfg.steps + 1);
    for s in 0..=cfg.steps {
        let lin = s as f32 / cfg.steps as f32;
        let warped = cfg.t_shift * lin / (1.0 - (1.0 - cfg.t_shift) * lin);
        schedule.push(warped);
    }

    let vocab = MASK as usize; // 1024
    let slots_01 = 2 * f;

    // 8 main diffusion passes for codebooks 0..2
    for step in 0..cfg.steps {
        let lp_tensor = network
            .target_log_probs(&tokens, f, &cache, 0, 2, None)
            .map_err(|e| format!("Diffusion step {step} failed: {e}"))?;
        let lp: Vec<f32> = lp_tensor
            .flatten_all()
            .and_then(|t| t.to_vec1())
            .map_err(|e| e.to_string())?;

        // Extract per-slot (confidence, best_prediction) across codebooks 0..2
        let mut confidence = vec![f32::NEG_INFINITY; slots_01];
        let mut prediction = vec![0u32; slots_01];
        for slot in 0..slots_01 {
            let row = &lp[slot * vocab..(slot + 1) * vocab];
            let mut best_val = f32::NEG_INFINITY;
            let mut best_idx = 0usize;
            for (idx, &v) in row.iter().enumerate() {
                if v > best_val {
                    best_val = v;
                    best_idx = idx;
                }
            }
            confidence[slot] = best_val;
            prediction[slot] = best_idx as u32;
        }

        let left: usize = (0..slots_01).filter(|&s| tokens[s] == MASK).count();
        let is_last = step == cfg.steps - 1;
        let quota = ((2 * f) as f32 * (schedule[step + 1] - schedule[step])).ceil() as usize;
        let k = if is_last { left } else { left.min(quota) };

        // Score masked slots with confidence - codebook_penalty + Gumbel noise
        let mut masked_scores: Vec<(f32, usize)> = Vec::with_capacity(left);
        for slot in 0..slots_01 {
            let u = rng.next_f32();
            let gumbel = -(-((u + 1e-10).ln()) + 1e-10).ln();
            if tokens[slot] == MASK {
                let cb = (slot / f) as f32;
                let pen = cfg.codebook_penalty * cb;
                let s = (confidence[slot] - pen) / cfg.position_temperature + gumbel;
                if s.is_finite() {
                    masked_scores.push((s, slot));
                }
            }
        }
        masked_scores.sort_unstable_by(|a, b| b.0.total_cmp(&a.0));

        let mut updated = tokens[..slots_01].to_vec();
        for &(_, slot) in masked_scores.iter().take(k) {
            updated[slot] = prediction[slot];
        }

        // Remasking: withdraw least-likely previously revealed slots
        if cfg.remasking && !is_last {
            let mut revealed_scores: Vec<(f32, usize)> = Vec::new();
            for slot in 0..slots_01 {
                let held = tokens[slot];
                if held != MASK {
                    let held_idx = (held as usize).min(vocab - 1);
                    let score = lp[slot * vocab + held_idx];
                    revealed_scores.push((score, slot));
                }
            }
            let withdraw = ((cfg.remask_ratio * k as f32).round() as usize).min(revealed_scores.len());
            if withdraw > 0 {
                revealed_scores.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
                for &(_, slot) in revealed_scores.iter().take(withdraw) {
                    updated[slot] = MASK;
                }
            }
        }
        tokens[..slots_01].copy_from_slice(&updated);
    }

    // Polish pass 1: codebooks 2..8 -> commit 2..4
    let first_tensor = network
        .target_log_probs(&tokens, f, &cache, 2, 8, Some(cfg.polish_bias))
        .map_err(|e| format!("Polish pass 1 failed: {e}"))?;
    let first: Vec<f32> = first_tensor
        .flatten_all()
        .and_then(|t| t.to_vec1())
        .map_err(|e| e.to_string())?;

    for cb_rel in 0..2 {
        let cb_abs = 2 + cb_rel;
        for t in 0..f {
            let row_start = (cb_rel * f + t) * vocab;
            let row = &first[row_start..row_start + vocab];
            tokens[cb_abs * f + t] = argmax(row) as u32;
        }
    }

    // Polish pass 2: codebooks 4..8 guided against pass 1's view of 4..8
    let second_tensor = network
        .target_log_probs(&tokens, f, &cache, 4, 8, Some(cfg.polish_bias))
        .map_err(|e| format!("Polish pass 2 failed: {e}"))?;
    let second: Vec<f32> = second_tensor
        .flatten_all()
        .and_then(|t| t.to_vec1())
        .map_err(|e| e.to_string())?;

    let g = cfg.polish_guidance;
    for cb_rel in 0..4 {
        let cb_abs = 4 + cb_rel;
        let first_cb_rel = 2 + cb_rel;
        for t in 0..f {
            let s_start = (cb_rel * f + t) * vocab;
            let f_start = (first_cb_rel * f + t) * vocab;
            let mut best_val = f32::NEG_INFINITY;
            let mut best_idx = 0usize;
            for c in 0..vocab {
                let sv = second[s_start + c];
                let fv = first[f_start + c];
                let guided = sv + g * (sv - fv);
                if guided > best_val {
                    best_val = guided;
                    best_idx = c;
                }
            }
            tokens[cb_abs * f + t] = best_idx as u32;
        }
    }

    Ok(tokens)
}

#[inline]
fn argmax(slice: &[f32]) -> usize {
    let mut best_val = f32::NEG_INFINITY;
    let mut best_idx = 0usize;
    for (i, &v) in slice.iter().enumerate() {
        if v > best_val {
            best_val = v;
            best_idx = i;
        }
    }
    best_idx
}
