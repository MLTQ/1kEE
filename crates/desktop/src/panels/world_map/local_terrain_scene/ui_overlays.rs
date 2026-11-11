use crate::theme;

use super::super::srtm_focus_cache;
use super::visual_half_extent_for_zoom;

pub(super) fn draw_legend(
    painter: &egui::Painter,
    rect: egui::Rect,
    title: &str,
    render_zoom: f32,
    active_body: crate::model::ActiveBody,
) {
    let interval_m = match active_body {
        crate::model::ActiveBody::Moon => super::super::srtm_focus_cache::zoom::lunar_spec_for_zoom(render_zoom).interval_m,
        crate::model::ActiveBody::Mars => super::super::srtm_focus_cache::zoom::mars_spec_for_zoom(render_zoom).interval_m,
        crate::model::ActiveBody::Earth => srtm_focus_cache::contour_interval_for_zoom(render_zoom),
    };
    // Sub-metre 3DEP tiers need a decimal; whole-metre tiers keep their
    // existing integer legend text.
    let interval_label = if (interval_m - interval_m.round()).abs() < f32::EPSILON {
        format!("{}", interval_m.round() as i64)
    } else {
        format!("{interval_m}")
    };
    let half_extent_km = visual_half_extent_for_zoom(render_zoom) * 111.32;
    painter.text(
        egui::pos2(rect.left() + 24.0, rect.bottom() - 86.0),
        egui::Align2::LEFT_TOP,
        format!(
            "{title}\nDRAG TO PAN · SHIFT+DRAG TO ORBIT · HOME FOR OVERHEAD\n{interval_label}M CONTOURS · {half_extent_km:.0}KM HALF-SPAN"
        ),
        egui::FontId::monospace(12.0),
        theme::text_muted(),
    );
}

/// Draw terrain pipeline and osmium extraction progress as stacked cards.
/// Terrain remains active until source loading and display preparation finish.
pub(super) fn draw_progress_overlay(
    painter: &egui::Painter,
    rect: egui::Rect,
    status: srtm_focus_cache::FocusContourRegionStatus,
    stages: super::super::contour_asset::LocalLoadingStages,
    preparing_tiles: usize,
    uploading_tiles: usize,
    composing: bool,
    osmium_progress: Option<(u32, u32)>,
    job_note: Option<&str>,
) {
    const CARD_W: f32 = 200.0;
    const CARD_H: f32 = 36.0;
    const GAP: f32 = 4.0;
    const RIGHT_MARGIN: f32 = 12.0;
    const BOTTOM_MARGIN: f32 = 12.0;

    let preparing = stages.preparing + preparing_tiles;
    let active = terrain_loading(status, stages, preparing_tiles, uploading_tiles, composing);
    let osmium_active = osmium_progress.is_some();
    if !active && !osmium_active {
        return;
    }
    let mut bottom_y = rect.bottom() - BOTTOM_MARGIN;
    if active {
        let frame = egui::Rect::from_min_size(
            egui::pos2(rect.right() - RIGHT_MARGIN - 320.0, bottom_y - 66.0),
            egui::vec2(320.0, 66.0),
        );
        let bar = egui::Rect::from_min_size(
            frame.left_bottom() + egui::vec2(0.0, -10.0),
            egui::vec2(frame.width(), 6.0),
        );
        let progress = if status.total_assets > 0 {
            status.ready_assets as f32 / status.total_assets as f32
        } else { 0.0 };
        let label = format!(
            "TERRAIN · SOURCE {}/{}{}\nBUILD {} · READ/DECODE {} · WAIT {}\nPREPARE {} · GPU UPLOAD {}",
            status.ready_assets, status.total_assets,
            if composing { " · COMPOSING" } else { "" },
            stages.building, stages.reading, stages.waiting,
            preparing, uploading_tiles,
        );
        draw_progress_card(painter, frame, bar, &label, progress.clamp(0.0, 0.99), theme::topo_color());
        bottom_y = frame.top() - GAP;
    }

    // ── Osmium cell-extraction card ────────────────────────────────────────
    if osmium_active {
        let (done, total) = osmium_progress.unwrap();
        let progress = if total > 0 {
            done as f32 / total as f32
        } else {
            0.0
        };
        // Truncate job note to fit in card width (≈26 chars at monospace 11)
        let label = if let Some(note) = job_note {
            let trimmed = note.trim_end_matches('…').trim_end_matches("...");
            if trimmed.len() > 28 {
                format!("{}…", &trimmed[..28])
            } else {
                trimmed.to_owned()
            }
        } else {
            format!("OSMIUM {done}/{total} cells")
        };
        let frame = egui::Rect::from_min_size(
            egui::pos2(rect.right() - RIGHT_MARGIN - CARD_W, bottom_y - CARD_H),
            egui::vec2(CARD_W, CARD_H),
        );
        let bar = egui::Rect::from_min_size(
            frame.left_bottom() + egui::vec2(0.0, -10.0),
            egui::vec2(frame.width(), 6.0),
        );
        draw_progress_card(
            painter,
            frame,
            bar,
            &label,
            progress,
            egui::Color32::from_rgb(160, 130, 50),
        );
    }
}

fn draw_progress_card(
    painter: &egui::Painter,
    frame: egui::Rect,
    bar: egui::Rect,
    label: &str,
    progress: f32,
    fill_color: egui::Color32,
) {
    painter.rect_filled(frame, 6.0, theme::panel_fill(208));
    painter.rect_stroke(
        frame,
        6.0,
        egui::Stroke::new(1.0, theme::panel_stroke()),
        egui::StrokeKind::Outside,
    );
    painter.text(
        frame.left_top() + egui::vec2(8.0, 6.0),
        egui::Align2::LEFT_TOP,
        label,
        egui::FontId::monospace(10.5),
        theme::text_muted(),
    );
    painter.rect_filled(bar, 3.0, theme::panel_fill(230).gamma_multiply(2.5));
    if progress > 0.0 {
        let filled = egui::Rect::from_min_max(
            bar.min,
            egui::pos2(bar.left() + bar.width() * progress, bar.bottom()),
        );
        painter.rect_filled(filled, 3.0, fill_color);
    }
}

pub(super) fn draw_empty_state(painter: &egui::Painter, rect: egui::Rect, label: &str) {
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::proportional(18.0),
        theme::text_muted(),
    );
}


fn terrain_loading(
    status: srtm_focus_cache::FocusContourRegionStatus,
    stages: super::super::contour_asset::LocalLoadingStages,
    preparing: usize,
    uploading: usize,
    composing: bool,
) -> bool {
    status.ready_assets < status.total_assets
        || stages.building + stages.reading + stages.preparing + stages.waiting > 0
        || preparing > 0 || uploading > 0 || composing
}

#[cfg(test)]
mod loading_tests {
    use super::*;
    #[test]
    fn completed_sources_do_not_hide_downstream_work() {
        let status = srtm_focus_cache::FocusContourRegionStatus {
            ready_assets: 9, total_assets: 9, pending_assets: 0,
        };
        let stages = super::super::super::contour_asset::LocalLoadingStages::default();
        assert!(!terrain_loading(status, stages, 0, 0, false));
        assert!(terrain_loading(status, stages, 2, 0, false));
        assert!(terrain_loading(status, stages, 0, 2, false));
        assert!(terrain_loading(status, stages, 0, 0, true));
        assert!(terrain_loading(status, super::super::super::contour_asset::LocalLoadingStages { reading: 2, ..stages }, 0, 0, false));
    }
}
