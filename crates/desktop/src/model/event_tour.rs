//! Severity-weighted, repeat-spaced stops from the complete live Factal feed.
use super::super::{EventRecord, EventSeverity};
use std::collections::{HashMap, HashSet, VecDeque};

const MAX_RECENT_STOPS: usize = 6;

struct Entry {
    event: EventRecord,
    credit: i64,
}

#[derive(Default)]
pub(super) struct EventTour {
    entries: Vec<Entry>,
    recent: VecDeque<String>,
    pending_start: bool,
}

impl EventTour {
    pub fn refresh(&mut self, events: &[EventRecord], current: Option<&str>) {
        // Preserve both waiting credit and recent visits by ID. A minute poll
        // must not reset coverage or force another visit to the latest headline.
        let mut credits: HashMap<_, _> = std::mem::take(&mut self.entries)
            .into_iter()
            .map(|entry| (entry.event.id, entry.credit))
            .collect();
        self.entries = ranked(events)
            .into_iter()
            .map(|event| Entry {
                credit: credits.remove(&event.id).unwrap_or(0),
                event,
            })
            .collect();
        self.recent
            .retain(|id| self.entries.iter().any(|entry| entry.event.id == *id));
        self.pending_start |= current.is_none();
    }

    pub fn take_pending(&mut self) -> Option<EventRecord> {
        if !self.pending_start {
            return None;
        }
        let event = self.next(None)?;
        self.pending_start = false;
        Some(event)
    }

    pub fn next(&mut self, current: Option<&str>) -> Option<EventRecord> {
        // With a large feed, leave six other stops before revisiting a story.
        // Smaller feeds shorten the cooldown so touring can still continue.
        let cooldown = MAX_RECENT_STOPS.min(self.entries.len().saturating_sub(1) / 2);
        let mut selected = None;
        let mut best_credit = i64::MIN;
        let mut total_weight = 0;
        for (index, entry) in self.entries.iter_mut().enumerate() {
            if Some(entry.event.id.as_str()) == current
                || self
                    .recent
                    .iter()
                    .rev()
                    .take(cooldown)
                    .any(|id| *id == entry.event.id)
            {
                continue;
            }
            // Smooth weighted round robin: all eligible stories accrue credit,
            // including low-priority ones. The winner pays for this turn, so
            // severity affects revisit frequency without starving the rest.
            let weight = weight(&entry.event);
            entry.credit += weight;
            total_weight += weight;
            if entry.credit > best_credit {
                selected = Some(index);
                best_credit = entry.credit;
            }
        }
        let entry = &mut self.entries[selected?];
        entry.credit -= total_weight;
        let event = entry.event.clone();
        self.recent.push_back(event.id.clone());
        if self.recent.len() > MAX_RECENT_STOPS {
            self.recent.pop_front();
        }
        Some(event)
    }

    pub fn position(&self, current: &str) -> Option<(usize, usize)> {
        self.entries
            .iter()
            .position(|entry| entry.event.id == current)
            .map(|i| (i + 1, self.entries.len()))
    }
}

fn ranked(events: &[EventRecord]) -> Vec<EventRecord> {
    let mut ordered: Vec<_> = events
        .iter()
        .filter(|e| {
            e.id.starts_with("factal-")
                && e.factal_brief.is_some()
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
        .cloned()
        .collect()
}

fn weight(event: &EventRecord) -> i64 {
    match severity(event) {
        i64::MIN..=2 => 1,
        3 => 3,
        4 => 6,
        _ => 10,
    }
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

#[cfg(test)]
#[path = "event_tour_tests.rs"]
mod tests;
