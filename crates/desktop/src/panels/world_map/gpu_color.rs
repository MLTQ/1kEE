//! Match egui's display-space colors at native GPU callback boundaries.
use eframe::wgpu;
use std::{collections::HashMap, sync::OnceLock};

pub(super) fn rgba(color: egui::Color32) -> [f32; 4] {
    color.to_array().map(|channel| channel as f32 / 255.0)
}

pub(super) fn shader(source: &str) -> wgpu::ShaderSource<'static> {
    wgpu::ShaderSource::Wgsl(format!("{}\n{source}", include_str!("color_output.wgsl")).into())
}

pub(super) fn fragment_options(
    format: wgpu::TextureFormat,
) -> wgpu::PipelineCompilationOptions<'static> {
    static CONSTANTS: OnceLock<[HashMap<String, f64>; 2]> = OnceLock::new();
    let constants = CONSTANTS.get_or_init(|| {
        [
            HashMap::from([("SRGB_FRAMEBUFFER".into(), 0.0)]),
            HashMap::from([("SRGB_FRAMEBUFFER".into(), 1.0)]),
        ]
    });
    wgpu::PipelineCompilationOptions {
        constants: &constants[usize::from(format.is_srgb())],
        ..Default::default()
    }
}
