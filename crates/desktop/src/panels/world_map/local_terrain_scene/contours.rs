//! Shared native local terrain rendering for Earth, Moon and Mars.
use super::*;
use crate::model::ActiveBody;

/// Classification is geometry; palette colors remain live draw uniforms.
fn classification(body: ActiveBody) -> (u64, i32) {
    match body {
        ActiveBody::Earth => (0, 50),
        ActiveBody::Moon => (1, 1_000),
        ActiveBody::Mars => (2, 1_000),
    }
}

/// Build full native tile batches asynchronously for any active body.
pub(super) fn batches(
    model: &AppModel,
    tiles: &[contour_asset::LocalTileGeometry],
    painter: &egui::Painter,
) -> Vec<local_contour_pass::LocalTileBatch> {
    if !model.show_contours {
        local_contour_pass::retain_instances(&std::collections::HashSet::new());
        return Vec::new();
    }

    // Resident geometry stores classification, not theme colors. The visible
    // callback supplies the current palette through uniforms on every frame.
    let (style_key, major_interval) = classification(model.active_body);

    let ctx = painter.ctx().clone();
    tiles
        .iter()
        .filter_map(|tile| {
            local_contour_pass::instances_for_tile(
                tile.id,
                &tile.contours,
                style_key,
                &ctx,
                move |contour| {
                    // Preserve the original body-specific contour hierarchy.
                    let major =
                        (contour.elevation_m.round() as i32).rem_euclid(major_interval) == 0;
                    (egui::Color32::WHITE, major)
                },
            )
        })
        .collect()
}

/// Draw the published surface with the exact projection used by tile residency.
pub(super) fn paint(
    painter: &egui::Painter,
    params: &projection::LocalProjectionParams,
    batches: &[local_contour_pass::LocalTileBatch],
    occlusion: bool,
    contour_stroke_scale: f32,
) {
    let pixels_per_point = painter.ctx().pixels_per_point();
    painter.add(
        local_contour_pass::LocalContourCallback::new(
            local_contour_pass::LocalContourPass::Surface,
            batches.to_vec(),
            params,
            1.0,
            gpu_contour_stroke_width_px(0.7, 1.0, contour_stroke_scale, pixels_per_point),
            gpu_contour_stroke_width_px(1.35, 1.0, contour_stroke_scale, pixels_per_point),
            painter.ctx().clone(),
        )
        .with_occlusion(occlusion)
        .with_palette(
            theme::hot_color(),
            theme::contour_color().gamma_multiply(0.78),
        )
        .into_paint_callback(painter.clip_rect()),
    );
}

#[cfg(test)]
#[path = "celestial_gpu_tests.rs"]
mod gpu_tests;
