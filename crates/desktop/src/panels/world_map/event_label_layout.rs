//! Stable screen-space slots for the cards shown during an event-tour stop.
use egui::{Pos2, Rect, Vec2};
use std::collections::BTreeMap;

const CARD_GAP: f32 = 6.0;
const EDGE_HYSTERESIS: f32 = 24.0;
const SLOT_GRACE_SECONDS: f64 = 2.0;

pub(super) struct Request<'a> {
    pub id: &'a str,
    pub anchor: Pos2,
    pub size: Vec2,
}

#[derive(Clone)]
struct Slot {
    rect: Rect,
    size: Vec2,
    last_seen: f64,
}

#[derive(Clone, Default)]
pub(super) struct Layout {
    bounds: Option<Rect>,
    slots: BTreeMap<String, Slot>,
}

impl Layout {
    pub fn begin_frame(&mut self, bounds: Rect, now: f64) {
        if self.bounds != Some(bounds) {
            self.slots.clear();
            self.bounds = Some(bounds);
        }
        self.slots
            .retain(|_, slot| now >= slot.last_seen && now - slot.last_seen <= SLOT_GRACE_SECONDS);
    }

    /// Existing cards survive small anchor excursions across the viewport edge;
    /// new cards still require an on-screen anchor. Scene horizon culling wins.
    pub fn accepts_anchor(&self, id: &str, anchor: Pos2, viewport: Rect) -> bool {
        viewport
            .expand(if self.slots.contains_key(id) {
                EDGE_HYSTERESIS
            } else {
                0.0
            })
            .contains(anchor)
    }

    pub fn place(&mut self, requests: &[Request<'_>], now: f64) -> Vec<Rect> {
        let bounds = self.bounds.expect("begin_frame before card placement");
        // A changed headline/font may need a larger card. Free only that slot;
        // reserve every unchanged card before considering any new placement.
        for request in requests {
            if self
                .slots
                .get(request.id)
                .is_some_and(|slot| slot.size != request.size)
            {
                self.slots.remove(request.id);
            }
        }
        let mut occupied: Vec<_> = self.slots.values().map(|slot| slot.rect).collect();
        requests
            .iter()
            .map(|request| {
                if let Some(slot) = self.slots.get_mut(request.id) {
                    slot.last_seen = now;
                    return slot.rect;
                }
                let rect = place_card(request.anchor, request.size, bounds, &occupied);
                occupied.push(rect);
                self.slots.insert(
                    request.id.to_owned(),
                    Slot {
                        rect,
                        size: request.size,
                        last_seen: now,
                    },
                );
                rect
            })
            .collect()
    }
}

/// Run only for new/resized cards, never just because a marker moved.
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
#[path = "event_label_layout_tests.rs"]
mod tests;
