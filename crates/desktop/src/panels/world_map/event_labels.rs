//! Persistent, non-interactive event cards for the automatic event tour.
use super::marker_style::MapMarker;
use crate::model::EventRecord;
use crate::theme;
use egui::{Rect, Vec2};
use std::collections::HashMap;

const PADDING: f32 = 8.0;
const FIELD_GAP: f32 = 4.0;

#[path = "event_label_layout.rs"]
mod layout;

pub(super) fn draw<'a>(
    painter: &egui::Painter,
    viewport: Rect,
    events: impl Iterator<Item = &'a EventRecord>,
    markers: &[MapMarker],
    opacity: f32,
) {
    let layout_id = egui::Id::new("follow_event_card_layout");
    let mut layout = painter.ctx().data_mut(|data| {
        data.remove_temp::<layout::Layout>(layout_id)
            .unwrap_or_default()
    });
    // Skip text lookup/layout and collision placement during wide transitions.
    // Dropping the old layout here lets the next stop choose fresh slots.
    if opacity <= 0.0 {
        return;
    }
    let bounds = viewport.shrink(PADDING);
    if bounds.width() <= PADDING * 2.0 || bounds.height() <= PADDING * 2.0 {
        return;
    }
    let now = painter.ctx().input(|input| input.time);
    layout.begin_frame(bounds, now);
    let markers: HashMap<_, _> = markers
        .iter()
        .filter(|marker| layout.accepts_anchor(&marker.id, marker.base, viewport))
        .map(|marker| (marker.id.as_str(), marker))
        .collect();
    let mut visible: Vec<_> = events
        .filter_map(|event| {
            markers
                .get(event.id.as_str())
                .map(|marker| (event, *marker))
        })
        .collect();
    // Polling can reorder records; stable ordering keeps overlapping cards from
    // trading places merely because the event payload was refreshed.
    visible.sort_unstable_by(|(a, _), (b, _)| a.id.cmp(&b.id));

    let mut painter = painter.with_clip_rect(viewport);
    painter.multiply_opacity(opacity);
    let style = painter.ctx().style();
    let width = 220.0_f32.min(bounds.width() - PADDING * 2.0);
    let mut requests = Vec::with_capacity(visible.len());
    let mut cards = Vec::with_capacity(visible.len());
    for (event, marker) in visible {
        let fields = [
            (
                event.severity.label(),
                egui::TextStyle::Body,
                event.severity.color(),
            ),
            (
                event.title.as_str(),
                egui::TextStyle::Body,
                style.visuals.strong_text_color(),
            ),
            (
                event.location_name.as_str(),
                egui::TextStyle::Small,
                theme::text_muted(),
            ),
        ];
        let text: Vec<_> = fields
            .into_iter()
            .map(|(text, font, color)| {
                painter.layout(text.to_owned(), font.resolve(&style), color, width)
            })
            .collect();
        let size = egui::vec2(
            text.iter()
                .map(|line| line.size().x)
                .fold(0.0_f32, f32::max)
                + PADDING * 2.0,
            text.iter().map(|line| line.size().y).sum::<f32>() + FIELD_GAP * 2.0 + PADDING * 2.0,
        );
        requests.push(layout::Request {
            id: &event.id,
            anchor: marker.base,
            size,
        });
        cards.push((marker.base, event.severity.color(), text));
    }
    let positions = layout.place(&requests, now);
    painter
        .ctx()
        .data_mut(|data| data.insert_temp(layout_id, layout));

    // Leaders sit beneath every card, and all shapes stay on the map's layer:
    // they cannot intercept navigation or cover the floating brief/feed windows.
    for (rect, (anchor, color, _)) in positions.iter().zip(&cards) {
        painter.line_segment(
            [*anchor, rect.clamp(*anchor)],
            egui::Stroke::new(1.0, color.gamma_multiply(0.65)),
        );
    }
    for (rect, (_, _, text)) in positions.into_iter().zip(cards) {
        painter.rect(
            rect,
            8.0,
            theme::panel_fill(238),
            egui::Stroke::new(1.0, theme::panel_stroke()),
            egui::StrokeKind::Inside,
        );
        let mut pos = rect.min + Vec2::splat(PADDING);
        for galley in text {
            let height = galley.size().y;
            painter.galley(pos, galley, style.visuals.text_color());
            pos.y += height + FIELD_GAP;
        }
    }
}

#[cfg(test)]
#[path = "event_labels_tests.rs"]
mod tests;
