use super::DirectoryCandidate;
use crate::model::CameraFeed;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

const DIRECTORY_CACHE_TTL: Duration = Duration::from_secs(30 * 60);
const ENRICHMENT_CACHE_TTL: Duration = Duration::from_secs(6 * 60 * 60);
const NEGATIVE_ENRICHMENT_CACHE_TTL: Duration = Duration::from_secs(30 * 60);
const CANCELLATION_POLL_INTERVAL: Duration = Duration::from_millis(50);

pub(super) struct RequestPacer {
    interval: Duration,
    next_start: Mutex<Instant>,
}

impl RequestPacer {
    pub(super) fn new(requests_per_minute: u16) -> Self {
        let requests_per_minute = requests_per_minute.max(1) as f64;
        Self {
            interval: Duration::from_secs_f64(60.0 / requests_per_minute),
            next_start: Mutex::new(Instant::now()),
        }
    }

    /// Waits for the next globally paced request slot. A short sleep slice
    /// keeps cancellation responsive without busy-waiting.
    pub(super) fn wait(&self, cancelled: &AtomicBool) -> bool {
        loop {
            if cancelled.load(Ordering::Acquire) {
                return false;
            }

            let remaining = {
                let mut next_start = self
                    .next_start
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let now = Instant::now();
                if now >= *next_start {
                    *next_start = now + self.interval;
                    return true;
                }
                next_start.saturating_duration_since(now)
            };
            thread::sleep(remaining.min(CANCELLATION_POLL_INTERVAL));
        }
    }
}

#[derive(Clone)]
struct Cached<T> {
    value: T,
    stored_at: Instant,
}

#[derive(Default)]
struct PipelineCache {
    directories: HashMap<String, Cached<Vec<DirectoryCandidate>>>,
    enrichments: HashMap<String, Cached<Option<CameraFeed>>>,
}

impl PipelineCache {
    fn prune_expired(&mut self) {
        let now = Instant::now();
        self.directories.retain(|_, entry| {
            now.saturating_duration_since(entry.stored_at) <= DIRECTORY_CACHE_TTL
        });
        self.enrichments.retain(|_, entry| {
            let ttl = if entry.value.is_some() {
                ENRICHMENT_CACHE_TTL
            } else {
                NEGATIVE_ENRICHMENT_CACHE_TTL
            };
            now.saturating_duration_since(entry.stored_at) <= ttl
        });
    }

    fn directory(&mut self, scope: &str) -> Option<(Vec<DirectoryCandidate>, Duration)> {
        let now = Instant::now();
        let hit = self.directories.get(scope).and_then(|entry| {
            let age = now.saturating_duration_since(entry.stored_at);
            (age <= DIRECTORY_CACHE_TTL).then(|| (entry.value.clone(), age))
        });
        if hit.is_none() {
            self.directories.remove(scope);
        }
        hit
    }

    fn store_directory(&mut self, scope: String, candidates: Vec<DirectoryCandidate>) {
        self.directories.insert(
            scope,
            Cached {
                value: candidates,
                stored_at: Instant::now(),
            },
        );
    }

    fn enrichment(&mut self, candidate: &DirectoryCandidate) -> Option<Option<CameraFeed>> {
        let key = candidate_cache_key(candidate);
        let now = Instant::now();
        let hit = self.enrichments.get(&key).and_then(|entry| {
            let age = now.saturating_duration_since(entry.stored_at);
            let ttl = if entry.value.is_some() {
                ENRICHMENT_CACHE_TTL
            } else {
                NEGATIVE_ENRICHMENT_CACHE_TTL
            };
            (age <= ttl).then(|| entry.value.clone())
        });
        if hit.is_none() {
            self.enrichments.remove(&key);
        }
        hit
    }

    fn store_enrichment(&mut self, candidate: &DirectoryCandidate, camera: Option<CameraFeed>) {
        self.enrichments.insert(
            candidate_cache_key(candidate),
            Cached {
                value: camera,
                stored_at: Instant::now(),
            },
        );
    }
}

pub(super) fn prune_expired() {
    cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .prune_expired();
}

pub(super) fn directory_scope_key(country_code: Option<&str>) -> String {
    country_code
        .filter(|code| !code.trim().is_empty())
        .map(|code| code.trim().to_ascii_uppercase())
        .unwrap_or_else(|| "global".to_owned())
}

pub(super) fn cached_directory(scope: &str) -> Option<(Vec<DirectoryCandidate>, Duration)> {
    cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .directory(scope)
}

pub(super) fn store_directory(scope: String, candidates: Vec<DirectoryCandidate>) {
    cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .store_directory(scope, candidates);
}

pub(super) fn cached_enrichment(candidate: &DirectoryCandidate) -> Option<Option<CameraFeed>> {
    cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .enrichment(candidate)
}

pub(super) fn store_enrichment(candidate: &DirectoryCandidate, camera: Option<CameraFeed>) {
    cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .store_enrichment(candidate, camera);
}

fn candidate_cache_key(candidate: &DirectoryCandidate) -> String {
    format!(
        "{}|{}|{}|{}|{}",
        candidate.camera_id,
        candidate.feed_url,
        candidate.detail_url,
        candidate.brand,
        candidate.location_hint
    )
}

fn cache() -> &'static Mutex<PipelineCache> {
    static CACHE: OnceLock<Mutex<PipelineCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(PipelineCache::default()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(feed_url: &str) -> DirectoryCandidate {
        DirectoryCandidate {
            camera_id: "42".into(),
            feed_url: feed_url.into(),
            detail_url: "http://www.insecam.org/en/view/42/".into(),
            brand: "Test".into(),
            location_hint: "Somewhere".into(),
        }
    }

    #[test]
    fn default_target_rate_is_converted_to_request_spacing() {
        let pacer = RequestPacer::new(250);
        assert_eq!(pacer.interval, Duration::from_millis(240));
    }

    #[test]
    fn cancelled_pacer_does_not_wait_for_a_request_slot() {
        let pacer = RequestPacer::new(10);
        let cancelled = AtomicBool::new(true);
        assert!(!pacer.wait(&cancelled));
    }

    #[test]
    fn candidate_cache_key_changes_when_the_advertised_feed_changes() {
        assert_ne!(
            candidate_cache_key(&candidate("http://8.8.8.8/a.jpg")),
            candidate_cache_key(&candidate("http://8.8.8.8/b.jpg"))
        );
    }

    #[test]
    fn negative_enrichment_results_are_reused() {
        let candidate = candidate("http://8.8.8.8/a.jpg");
        let mut cache = PipelineCache::default();
        cache.store_enrichment(&candidate, None);

        assert!(matches!(cache.enrichment(&candidate), Some(None)));
    }

    #[test]
    fn directory_scope_keys_are_stable() {
        assert_eq!(directory_scope_key(None), "global");
        assert_eq!(directory_scope_key(Some(" us ")), "US");
    }
}
