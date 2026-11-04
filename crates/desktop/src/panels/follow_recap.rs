//! Broadcast recap identification and elapsed-day clock, including OBS view.
use crate::model::follow_recap::RegionalRecap;
use crate::theme;
use egui::{Align2, FontId, Rect};

pub(super) fn draw(painter: &egui::Painter, viewport: Rect, recap: &RegionalRecap) {
    let rect = Rect::from_min_size(
        viewport.left_top() + egui::vec2(12.0, 12.0),
        egui::vec2((viewport.width() - 24.0).min(440.0), 132.0),
    );
    if rect.width() < 200.0 || !viewport.contains_rect(rect) {
        return;
    }
    let painter = painter.with_clip_rect(rect);
    painter.rect_filled(rect, 8.0, theme::panel_fill(232));
    let text = painter.ctx().style().visuals.strong_text_color();
    let label = |y, size, value: &str, color| {
        painter.text(
            rect.left_top() + egui::vec2(14.0, y),
            Align2::LEFT_TOP,
            value,
            FontId::proportional(size),
            color,
        );
    };
    label(
        10.0,
        14.0,
        &format!("24hr recap · {}", recap.region.name()),
        text,
    );
    painter.text(
        rect.left_top() + egui::vec2(14.0, 31.0),
        Align2::LEFT_TOP,
        recap.clock(),
        FontId::monospace(28.0),
        text,
    );
    let window = format!(
        "{} to {} UTC",
        crate::event_store::unix_to_datetime_str(recap.replay.sim_from),
        crate::event_store::unix_to_datetime_str(recap.replay.sim_to)
    );
    label(66.0, 11.0, &window, theme::text_muted());
    let status = if recap.loading() {
        "Loading stored events…".to_owned()
    } else if recap.framing() {
        "Framing region…".to_owned()
    } else if recap.replay.events.is_empty() {
        "No stored events in this region · Returning to live shortly".to_owned()
    } else {
        format!(
            "{} / {} events · Then back to live",
            recap.replay.revealed_events().len(),
            recap.replay.events.len()
        )
    };
    label(87.0, 12.0, &status, text);
    let track = Rect::from_min_size(
        rect.left_top() + egui::vec2(14.0, 116.0),
        egui::vec2(rect.width() - 28.0, 4.0),
    );
    painter.rect_filled(track, 2.0, theme::panel_stroke());
    painter.rect_filled(
        Rect::from_min_size(
            track.min,
            egui::vec2(track.width() * recap.replay.progress(), track.height()),
        ),
        2.0,
        theme::topo_color(),
    );
    painter
        .ctx()
        .request_repaint_after(std::time::Duration::from_millis(33));
}
