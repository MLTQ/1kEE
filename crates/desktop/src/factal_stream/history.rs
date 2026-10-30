use super::api::{FetchError, RateLimitPolicy, fetch_events_since};
use crate::model::AppModel;
use std::sync::{Mutex, OnceLock};
use std::thread::{self, JoinHandle};

struct HistoryManager {
    active: Option<JoinHandle<Result<usize, FetchError>>>,
}

fn manager() -> &'static Mutex<HistoryManager> {
    static MANAGER: OnceLock<Mutex<HistoryManager>> = OnceLock::new();
    MANAGER.get_or_init(|| Mutex::new(HistoryManager { active: None }))
}

/// Replay backfill is explicit and independent of the live polling schedule.
pub fn trigger_history_fetch(api_key: String, days_back: u32) {
    let mut manager = manager().lock().unwrap();
    if manager.active.is_some() {
        return;
    }
    manager.active = Some(thread::spawn(move || {
        let from = crate::event_store::now_unix() - i64::from(days_back) * 86_400;
        let outcome = fetch_events_since(
            &api_key,
            from,
            RateLimitPolicy::Retry,
            crate::event_store::upsert_events,
        )
        .map(|stats| stats.events);
        crate::app::request_repaint();
        outcome
    }));
}

pub fn history_tick(model: &mut AppModel) {
    let finished = {
        let mut manager = manager().lock().unwrap();
        if manager
            .active
            .as_ref()
            .is_some_and(|handle| handle.is_finished())
        {
            manager.active.take()
        } else {
            None
        }
    };
    if let Some(handle) = finished {
        match handle.join() {
            Ok(Ok(count)) => {
                model.replay_history_status = format!("{count} historical events loaded");
                model.push_log(format!(
                    "Factal history fetch complete: {count} unique event(s) loaded."
                ));
                if model.replay_mode {
                    model.rebuild_replay_state();
                }
            }
            Ok(Err(FetchError::Auth)) => {
                model.replay_history_status = "Auth error during history fetch".into();
            }
            Ok(Err(FetchError::RateLimited)) => {
                model.replay_history_status =
                    "History fetch rate limited; reopen replay to retry".into();
            }
            Ok(Err(FetchError::Other(error))) => {
                model.replay_history_status = format!("History fetch error: {error}");
            }
            Err(_) => {
                model.replay_history_status = "History fetch panicked".into();
            }
        }
    }
}

pub fn is_history_fetching() -> bool {
    manager().lock().unwrap().active.is_some()
}
