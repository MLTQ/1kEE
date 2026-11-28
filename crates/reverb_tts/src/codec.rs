//! Decoder-only implementation of `HiggsAudioV2TokenizerModel` (RVQ + DAC upsampler).
//! Loads only the ~110 MB decoder tensors from `higgs-audio-v2-tokenizer/model.safetensors`,
//! skipping the ~650 MB HuBERT semantic model and acoustic encoder completely.

use candle_core::{DType, Device, Result, Tensor};
use safetensors::{Dtype as StDtype, SafeTensors};
use std::path::Path;

use crate::model::CODEBOOKS;

pub const SAMPLE_RATE: usize = 24_000;
const UPSAMPLING_RATIOS: [usize; 5] = [8, 5, 4, 2, 3];

struct Snake1d {
    alpha: Tensor, // [1, C, 1] F32
}

impl Snake1d {
    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        let sin_sq = x.broadcast_mul(&self.alpha)?.sin()?.sqr()?;
        let denom = (&self.alpha + 1e-9)?;
        x + sin_sq.broadcast_div(&denom)?
    }
}

struct Conv1d {
    weight: Tensor, // [out, in, k]
    bias: Tensor,   // [1, out, 1]
    padding: usize,
    dilation: usize,
}

impl Conv1d {
    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        x.conv1d(&self.weight, self.padding, 1, self.dilation, 1)?
            .broadcast_add(&self.bias)
    }
}

struct ConvTranspose1d {
    weight: Tensor, // [in, out, k]
    bias: Tensor,   // [1, out, 1]
    stride: usize,
    padding: usize,
    output_padding: usize,
}

impl ConvTranspose1d {
    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        x.conv_transpose1d(
            &self.weight,
            self.padding,
            self.output_padding,
            self.stride,
            1,
            1,
        )?
        .broadcast_add(&self.bias)
    }
}

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

struct DacDecoderBlock {
    snake1: Snake1d,
    conv_t1: ConvTranspose1d,
    res_units: [DacResUnit; 3],
}

impl DacDecoderBlock {
    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        let mut h = self.conv_t1.forward(&self.snake1.forward(x)?)?;
        for unit in &self.res_units {
            h = unit.forward(&h)?;
        }
        Ok(h)
    }
}

struct QuantizerLayer {
    embed: Tensor,         // [1024, 64]
    project_out_t: Tensor, // [64, 1024]
    project_out_b: Tensor, // [1, 1024]
}

impl QuantizerLayer {
    fn decode(&self, indices: &[u32], device: &Device) -> Result<Tensor> {
        let idx = Tensor::from_slice(indices, indices.len(), device)?;
        let q = self.embed.index_select(&idx, 0)?; // [F, 64]
        q.matmul(&self.project_out_t)?
            .broadcast_add(&self.project_out_b) // [F, 1024]
    }
}

pub struct HiggsDecoder {
    quantizers: Vec<QuantizerLayer>,
    fc2_t: Tensor, // [1024, 256]
    fc2_b: Tensor, // [1, 256]
    conv1: Conv1d,
    blocks: Vec<DacDecoderBlock>,
    final_snake: Snake1d,
    conv2: Conv1d,
    device: Device,
}

impl HiggsDecoder {
    pub fn load(codec_dir: &Path, device: &Device) -> std::result::Result<Self, String> {
        let path = codec_dir.join("model.safetensors");
        let bytes = std::fs::read(&path)
            .map_err(|e| format!("Cannot read HiggsAudioV2 codec {}: {e}", path.display()))?;
        let st = SafeTensors::deserialize(&bytes)
            .map_err(|e| format!("Invalid HiggsAudioV2 safetensors: {e}"))?;

        let load_f32 = |name: &str| -> std::result::Result<Tensor, String> {
            let view = st
                .tensor(name)
                .map_err(|e| format!("Missing codec tensor {name}: {e}"))?;
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
                other => return Err(format!("Unsupported codec dtype {other:?} for {name}")),
            };
            Tensor::from_vec(vals, view.shape(), device).map_err(|e| e.to_string())
        };
        let load_bias_1d = |name: &str| -> std::result::Result<Tensor, String> {
            let b = load_f32(name)?;
            let c = b.dim(0).map_err(|e| e.to_string())?;
            b.reshape((1, c, 1)).map_err(|e| e.to_string())
        };

        let mut quantizers = Vec::with_capacity(CODEBOOKS);
        for i in 0..CODEBOOKS {
            let p = format!("quantizer.quantizers.{i}");
            let embed = load_f32(&format!("{p}.codebook.embed"))?;
            let w = load_f32(&format!("{p}.project_out.weight"))?
                .t()
                .and_then(|t| t.contiguous())
                .map_err(|e| e.to_string())?;
            let b = load_f32(&format!("{p}.project_out.bias"))?
                .unsqueeze(0)
                .map_err(|e| e.to_string())?;
            quantizers.push(QuantizerLayer {
                embed,
                project_out_t: w,
                project_out_b: b,
            });
        }

        let fc2_t = load_f32("fc2.weight")?
            .t()
            .and_then(|t| t.contiguous())
            .map_err(|e| e.to_string())?;
        let fc2_b = load_f32("fc2.bias")?
            .unsqueeze(0)
            .map_err(|e| e.to_string())?;

        let conv1 = Conv1d {
            weight: load_f32("acoustic_decoder.conv1.weight")?,
            bias: load_bias_1d("acoustic_decoder.conv1.bias")?,
            padding: 3,
            dilation: 1,
        };

        let mut blocks = Vec::with_capacity(UPSAMPLING_RATIOS.len());
        for (i, &stride) in UPSAMPLING_RATIOS.iter().enumerate() {
            let bp = format!("acoustic_decoder.block.{i}");
            let snake1 = Snake1d {
                alpha: load_f32(&format!("{bp}.snake1.alpha"))?,
            };
            // Matches HiggsAudioV2TokenizerModel._adjust_dac_decoder:
            // padding = ceil(stride / 2) = (stride + 1) / 2, output_padding = stride % 2
            let conv_t1 = ConvTranspose1d {
                weight: load_f32(&format!("{bp}.conv_t1.weight"))?,
                bias: load_bias_1d(&format!("{bp}.conv_t1.bias"))?,
                stride,
                padding: stride.div_ceil(2),
                output_padding: stride % 2,
            };
            let load_res = |unit_idx: usize, dilation: usize| -> std::result::Result<DacResUnit, String> {
                let rp = format!("{bp}.res_unit{unit_idx}");
                Ok(DacResUnit {
                    snake1: Snake1d {
                        alpha: load_f32(&format!("{rp}.snake1.alpha"))?,
                    },
                    conv1: Conv1d {
                        weight: load_f32(&format!("{rp}.conv1.weight"))?,
                        bias: load_bias_1d(&format!("{rp}.conv1.bias"))?,
                        padding: 3 * dilation,
                        dilation,
                    },
                    snake2: Snake1d {
                        alpha: load_f32(&format!("{rp}.snake2.alpha"))?,
                    },
                    conv2: Conv1d {
                        weight: load_f32(&format!("{rp}.conv2.weight"))?,
                        bias: load_bias_1d(&format!("{rp}.conv2.bias"))?,
                        padding: 0,
                        dilation: 1,
                    },
                })
            };
            let res_units = [load_res(1, 1)?, load_res(2, 3)?, load_res(3, 9)?];
            blocks.push(DacDecoderBlock {
                snake1,
                conv_t1,
                res_units,
            });
        }

        let final_snake = Snake1d {
            alpha: load_f32("acoustic_decoder.snake1.alpha")?,
        };
        let conv2 = Conv1d {
            weight: load_f32("acoustic_decoder.conv2.weight")?,
            bias: load_bias_1d("acoustic_decoder.conv2.bias")?,
            padding: 3,
            dilation: 1,
        };

        Ok(Self {
            quantizers,
            fc2_t,
            fc2_b,
            conv1,
            blocks,
            final_snake,
            conv2,
            device: device.clone(),
        })
    }

    /// Decode `[8, frames]` row-major codec tokens into mono 24 kHz float32 audio samples.
    pub fn decode(&self, codes: &[u32], frames: usize) -> std::result::Result<Vec<f32>, String> {
        if codes.len() != CODEBOOKS * frames || frames == 0 {
            return Err(format!(
                "Invalid codec token length {} for {frames} frames",
                codes.len()
            ));
        }
        let mut quantized: Option<Tensor> = None;
        for cb in 0..CODEBOOKS {
            let row = &codes[cb * frames..(cb + 1) * frames];
            let q = self.quantizers[cb]
                .decode(row, &self.device)
                .map_err(|e| format!("RVQ decode cb {cb} failed: {e}"))?;
            quantized = Some(match quantized {
                None => q,
                Some(acc) => (acc + q).map_err(|e| e.to_string())?,
            });
        }
        let q = quantized.unwrap(); // [F, 1024]
        let x = q
            .matmul(&self.fc2_t)
            .and_then(|t| t.broadcast_add(&self.fc2_b))
            .and_then(|t| t.t())
            .and_then(|t| t.unsqueeze(0))
            .and_then(|t| t.contiguous())
            .map_err(|e| format!("Codec fc2 failed: {e}"))?; // [1, 256, F]

        let mut h = self
            .conv1
            .forward(&x)
            .map_err(|e| format!("Codec conv1 failed: {e}"))?;
        for (i, block) in self.blocks.iter().enumerate() {
            h = block
                .forward(&h)
                .map_err(|e| format!("Codec block {i} failed: {e}"))?;
        }
        let h = self
            .final_snake
            .forward(&h)
            .and_then(|t| self.conv2.forward(&t))
            .map_err(|e| format!("Codec output conv failed: {e}"))?;
        // Note: HiggsAudioV2Tokenizer replaces final Tanh with Identity.
        h.to_dtype(DType::F32)
            .and_then(|t| t.flatten_all())
            .and_then(|t| t.to_vec1())
            .map_err(|e| format!("Codec readback failed: {e}"))
    }
}
