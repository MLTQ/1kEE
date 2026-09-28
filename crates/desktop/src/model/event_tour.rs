//! Ranked stops from the latest successful Factal payload.
use super::super::{EventRecord, EventSeverity};
use std::collections::HashSet;

const MAX_STOPS: usize = 6;

#[derive(Default)]
pub(super) struct EventTour {
    entries: Vec<EventRecord>,
    next_index: usize,
    pending: Option<EventRecord>,
}

impl EventTour {
    pub fn refresh(&mut self, events: &[EventRecord], current: Option<&str>) {
        let entries = ranked(events);
        let head_changed = self.entries.first().map(|e| &e.id) != entries.first().map(|e| &e.id);
        let old_next = self.entries.get(self.next_index).map(|e| e.id.clone());
        self.entries = entries;
        if head_changed {
            // A new highest-priority item takes over smoothly on the next tick.
            // The same head/current ID must never restart its flight or dwell.
            self.pending = self
                .entries
                .first()
                .filter(|e| Some(e.id.as_str()) != current)
                .cloned();
            self.next_index = usize::from(self.entries.len() > 1);
        } else {
            // Refresh metadata/ranking without repeatedly sending the camera
            // back to stop one and starving the remaining stops on every poll.
            self.next_index = self
                .entries
                .iter()
                .position(|e| Some(e.id.as_str()) == current)
                .map(|i| (i + 1) % self.entries.len())
                .or_else(|| {
                    self.entries
                        .iter()
                        .position(|e| Some(&e.id) == old_next.as_ref())
                })
                .unwrap_or(0);
            self.pending = self
                .pending
                .take()
                .and_then(|pending| self.entries.iter().find(|e| e.id == pending.id).cloned());
        }
    }

    pub fn take_pending(&mut self) -> Option<EventRecord> {
        self.pending.take()
    }

    pub fn next(&mut self, current: Option<&str>) -> Option<EventRecord> {
        for _ in 0..self.entries.len() {
            let i = self.next_index % self.entries.len();
            self.next_index = (i + 1) % self.entries.len();
            let event = &self.entries[i];
            if Some(event.id.as_str()) != current {
                return Some(event.clone());
            }
        }
        None
    }

    pub fn position(&self, current: &str) -> Option<(usize, usize)> {
        self.entries
            .iter()
            .position(|e| e.id == current)
            .map(|i| (i + 1, self.entries.len()))
    }
}

fn ranked(events: &[EventRecord]) -> Vec<EventRecord> {
    let mut ordered: Vec<_> = events
        .iter()
        .filter(|e| {
            e.factal_brief.is_some()
                && e.location.lat.is_finite()
                && e.location.lon.is_finite()
                && e.location.lat.abs() <= 90.0
                && e.location.lon.abs() <= 180.0
        })
        .collect();
    ordered.sort_by(|a, b| {
        severity(b)
            .cmp(&severity(a))
            .then_with(|| timestamp(b).cmp(&timestamp(a)))
            .then_with(|| a.id.cmp(&b.id))
    });
    let mut unique = HashSet::new();
    ordered
        .into_iter()
        .filter(|e| unique.insert(e.id.as_str()))
        .take(MAX_STOPS)
        .cloned()
        .collect()
}

fn severity(event: &EventRecord) -> i64 {
    event
        .factal_brief
        .as_ref()
        .and_then(|b| b.severity_value)
        .unwrap_or(match event.severity {
            EventSeverity::Critical => 4,
            EventSeverity::Elevated => 2,
            EventSeverity::Advisory => 1,
        })
}

fn timestamp(event: &EventRecord) -> i64 {
    event
        .factal_brief
        .as_ref()
        .and_then(|b| b.occurred_at_raw.as_deref())
        .filter(|s| s.is_ascii() && s.len() >= 10)
        .and_then(crate::event_store::parse_iso_to_unix)
        .unwrap_or(0)
}
