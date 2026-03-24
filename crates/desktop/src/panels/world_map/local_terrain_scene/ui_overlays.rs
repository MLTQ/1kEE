use crate::theme;

use super::visual_half_extent_for_zoom;
use super::super::srtm_focus_cache;

pub(super) fn draw_legend(painter: &egui::Painter, rect: egui::Rect, title: &str, render_zoom: f32) {
    let interval_m = srtm_focus_cache::contour_interval_for_zoom(render_zoom);
    let half_extent_km = visual_half_extent_for_zoom(render_zoom) * 111.32;
    painter.text(
        egui::pos2(rect.left() + 24.0, rect.bottom() - 86.0),
        egui::Align2::LEFT_TOP,
        format!(
            "{title}\nFIXED OBLIQUE CAMERA\n{interval_m}M CONTOURS · {half_extent_km:.0}KM HALF-SPAN"
        ),
        egui::FontId::monospace(12.0),
        theme::text_muted(),
    );
}

/// Draw the bottom-right progress overlay.  Handles SRTM cache progress and
/// osmium cell-extraction progress as stacked cards; each card is only shown
/// when its data is available so they coexist without gaps when both are active.
pub(super) fn draw_progress_overlay(
    painter: &egui::Painter,
    rect: egui::Rect,
    cache_status: Option<srtm_focus_cache::FocusContourRegionStatus>,
    osmium_progress: Option<(u32, u32)>,
    job_note: Option<&str>,
) {
    const CARD_W: f32 = 200.0;
    const CARD_H: f32 = 36.0;
    const GAP: f32 = 4.0;
    const RIGHT_MARGIN: f32 = 12.0;
    const BOTTOM_MARGIN: f32 = 12.0;

    let cache_active = cache_status
        .map(|s| s.total_assets > 0 && s.ready_assets < s.total_assets)
        .unwrap_or(false);
    let osmium_active = osmium_progress.is_some();

    if !cache_active && !osmium_active {
        return;
    }

    // Cards stack upward from the bottom.  Cache bar is always on bottom when both visible.
    let mut bottom_y = rect.bottom() - BOTTOM_MARGIN;

    // ── SRTM cache card ────────────────────────────────────────────────────
    if cache_active {
        let status = cache_status.unwrap();
        let progress = (status.ready_assets as f32 / status.total_assets as f32).clamp(0.0, 1.0);
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
            &format!(
                "CACHE {} / {}  ·  {} PENDING",
                status.ready_assets, status.total_assets, status.pending_assets
            ),
            progress,
            theme::topo_color(),
        );
        bottom_y = frame.top() - GAP;
    }

    // ── Osmium cell-extraction card ────────────────────────────────────────
    if osmium_active {
        let (done, total) = osmium_progress.unwrap();
        let progress = if total > 0 { done as f32 / total as f32 } else { 0.0 };
        // Truncate job note to fit in card width (≈26 chars at monospace 11)
        let label = if let Some(note) = job_note {
            let trimmed = note.trim_end_matches('…').trim_end_matches("...");
            if trimmed.len() > 28 { format!("{}…", &trimmed[..28]) } else { trimmed.to_owned() }
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
