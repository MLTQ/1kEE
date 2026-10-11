//! Persistent, non-interactive event cards for the automatic event tour.
use super::marker_style::MapMarker;
use crate::model::EventRecord;
use crate::theme;
use egui::{Pos2, Rect, Vec2};
use std::collections::HashMap;

const PADDING: f32 = 8.0;
const FIELD_GAP: f32 = 4.0;
const CARD_GAP: f32 = 6.0;

pub(super) fn draw<'a>(
    painter: &egui::Painter,
    viewport: Rect,
    events: impl Iterator<Item = &'a EventRecord>,
    markers: &[MapMarker],
) {
    let bounds = viewport.shrink(PADDING);
    if bounds.width() <= PADDING * 2.0 || bounds.height() <= PADDING * 2.0 {
        return;
    }
    let markers: HashMap<_, _> = markers
        .iter()
        .filter(|marker| viewport.contains(marker.base))
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

    let painter = painter.with_clip_rect(viewport);
    let style = painter.ctx().style();
    let width = 220.0_f32.min(bounds.width() - PADDING * 2.0);
    let mut occupied = Vec::with_capacity(visible.len());
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
        let rect = place_card(marker.base, size, bounds, &occupied);
        occupied.push(rect);
        cards.push((rect, marker.base, event.severity.color(), text));
    }

    // Leaders sit beneath every card, and all shapes stay on the map's layer:
    // they cannot intercept navigation or cover the floating brief/feed windows.
    for (rect, anchor, color, _) in &cards {
        painter.line_segment(
            [*anchor, rect.clamp(*anchor)],
            egui::Stroke::new(1.0, color.gamma_multiply(0.65)),
        );
    }
    for (rect, _, _, text) in cards {
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

/// Prefer nearby space without collisions. A fixed candidate count bounds work
/// during camera motion; crowded views still draw every card, without a cap.
fn place_card(anchor: Pos2, size: Vec2, bounds: Rect, occupied: &[Rect]) -> Rect {
    let fit = |pos: Pos2| {
        Rect::from_min_size(
            egui::pos2(
                pos.x
                    .clamp(bounds.left(), (bounds.right() - size.x).max(bounds.left())),
                pos.y
                    .clamp(bounds.top(), (bounds.bottom() - size.y).max(bounds.top())),
            ),
            size,
        )
    };
    let mut best = fit(anchor + egui::vec2(14.0, -8.0));
    let mut best_score = (f32::INFINITY, f32::INFINITY);
    for column in 0..3 {
        for row in [0, -1, 1, -2, 2, -3, 3] {
            for right in [true, false] {
                let offset = 14.0 + column as f32 * (size.x + CARD_GAP);
                let x = if right {
                    anchor.x + offset
                } else {
                    anchor.x - offset - size.x
                };
                let y = anchor.y - 8.0 + row as f32 * (size.y + CARD_GAP);
                let candidate = fit(egui::pos2(x, y));
                let overlap: f32 = occupied
                    .iter()
                    .map(|rect| {
                        let intersection = rect.expand(CARD_GAP).intersect(candidate);
                        if intersection.is_positive() {
                            intersection.area()
                        } else {
                            0.0
                        }
                    })
                    .sum();
                let score = (overlap, candidate.center().distance_sq(anchor));
                if score < best_score {
                    best = candidate;
                    best_score = score;
                }
            }
        }
    }
    best
}

#[cfg(test)]
#[path = "event_labels_tests.rs"]
mod tests;
