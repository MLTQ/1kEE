use crate::model::{AppModel, EventRecord};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

mod api;
mod history;
mod parse;

use api::{FetchError, Snapshot};
pub use history::{history_tick, is_history_fetching, trigger_history_fetch};

const POLL_INTERVAL: Duration = Duration::from_secs(60);

struct PollManager {
    active: Option<ActivePoll>,
    generation: u64,
    tracked_key: String,
    next_poll_at: Option<Instant>,
    shutdown: bool,
}

struct ActivePoll {
    generation: u64,
    handle: JoinHandle<Result<Snapshot, FetchError>>,
}

impl PollManager {
    fn poll_due(&self, now: Instant) -> bool {
        !self.shutdown && self.active.is_none() && self.next_poll_at.is_some_and(|next| now >= next)
    }

    fn poll_finished(&mut self, generation: u64, now: Instant) {
        // A manual refresh/key change keeps its immediate request pending.
        if self.generation == generation {
            self.next_poll_at = Some(now + POLL_INTERVAL);
        }
    }
}

pub fn tick(model: &mut AppModel) {
    let key = model.factal_api_key.trim().to_owned();
    let now = Instant::now();
    let finished = {
        let mut manager = manager().lock().unwrap();
        if manager.tracked_key != key {
            manager.tracked_key = key.clone();
            manager.generation = manager.generation.wrapping_add(1);
            manager.next_poll_at = Some(now);
            manager.shutdown = false;
        }
        if manager
            .active
            .as_ref()
            .is_some_and(|active| active.handle.is_finished())
        {
            let active = manager.active.take().unwrap();
            manager.poll_finished(active.generation, now);
            Some(active)
        } else {
            None
        }
    };

    if let Some(active) = finished {
        match active.handle.join() {
            Ok(outcome) => apply_outcome(model, active.generation, outcome),
            Err(_) => {
                if active.generation == current_generation() {
                    model.factal_stream_status = "error".into();
                    model.push_log("Factal poll worker panicked before returning data.".into());
                }
            }
        }
    }

    if key.is_empty() {
        model.factal_stream_status = "inactive".into();
        return;
    }

    let mut manager = manager().lock().unwrap();
    if manager.poll_due(now) {
        let handle = thread::spawn(move || {
            let outcome = api::fetch_live_events(&key);
            crate::app::request_repaint();
            outcome
        });
        manager.active = Some(ActivePoll {
            generation: manager.generation,
            handle,
        });
        manager.next_poll_at = None;
        if model.factal_stream_status != "live" {
            model.factal_stream_status = "syncing".into();
        }
    }
}

pub fn invalidate() {
    let mut manager = manager().lock().unwrap();
    manager.generation = manager.generation.wrapping_add(1);
    manager.next_poll_at = Some(Instant::now());
    manager.shutdown = false;
}

pub fn shutdown() {
    let mut manager = manager().lock().unwrap();
    manager.generation = manager.generation.wrapping_add(1);
    manager.shutdown = true;
    manager.next_poll_at = None;
}

fn apply_outcome(model: &mut AppModel, generation: u64, outcome: Result<Snapshot, FetchError>) {
    if generation != current_generation() {
        return;
    }

    match outcome {
        Ok(snapshot) => apply_snapshot(model, snapshot),
        Err(FetchError::Auth) => {
            let was_status = model.factal_stream_status.clone();
            model.factal_stream_status = "auth error".into();
            if was_status != "auth error" {
                model.push_log(
                    "Factal API rejected the configured token; check the API key.".into(),
                );
            }
        }
        Err(FetchError::RateLimited) => {
            let was_status = model.factal_stream_status.clone();
            model.factal_stream_status = "rate limited".into();
            if was_status != "rate limited" {
                model.push_log(
                    "Factal API rate limited the live poll; keeping the previous events and retrying after one minute."
                        .into(),
                );
            }
        }
        Err(FetchError::Other(error)) => {
            model.factal_stream_status = "error".into();
            model.push_log(format!(
                "Factal poll failed; keeping the previous events: {error}"
            ));
        }
    }
}

fn apply_snapshot(model: &mut AppModel, snapshot: Snapshot) {
    let previous: HashMap<_, _> = model
        .events
        .iter()
        .filter(|event| event.id.starts_with("factal-"))
        .map(|event| (event.id.as_str(), event))
        .collect();
    let added = snapshot
        .events
        .iter()
        .filter(|event| !previous.contains_key(event.id.as_str()))
        .count();
    let updated = snapshot
        .events
        .iter()
        .filter(|event| {
            previous
                .get(event.id.as_str())
                .is_some_and(|old| raw_payload(old) != raw_payload(event))
        })
        .count();
    let removed = previous.len() + added - snapshot.events.len();
    let should_log = model.factal_stream_status != "live" || added + updated + removed > 0;
    let count = snapshot.events.len();
    crate::event_store::upsert_events(&snapshot.events);
    model.replace_factal_events(snapshot.events);
    model.factal_stream_status = "live".into();
    if should_log {
        model.push_log(format!(
            "Factal sync: {added} new, {updated} updated, {removed} removed; {count} geolocated event(s) in the past 24h ({} items across {} page(s)).",
            snapshot.stats.items, snapshot.stats.pages
        ));
    }
}

fn raw_payload(event: &EventRecord) -> Option<&str> {
    event
        .factal_brief
        .as_ref()
        .map(|brief| brief.raw_json_pretty.as_str())
}

fn current_generation() -> u64 {
    manager().lock().unwrap().generation
}

fn manager() -> &'static Mutex<PollManager> {
    static MANAGER: OnceLock<Mutex<PollManager>> = OnceLock::new();
    MANAGER.get_or_init(|| {
        Mutex::new(PollManager {
            active: None,
            generation: 0,
            tracked_key: String::new(),
            next_poll_at: Some(Instant::now()),
            shutdown: false,
        })
    })
}

#[cfg(test)]
#[path = "factal_stream/tests.rs"]
mod tests;
