//! Measured work completion, never elapsed-time estimates.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

type Key = (PathBuf, i32, i32, i32);
pub type BuildSnapshot = HashMap<(i32, i32), Arc<BuildProgress>>;

fn registry() -> &'static Mutex<HashMap<Key, Weak<BuildProgress>>> {
    static REGISTRY: OnceLock<Mutex<HashMap<Key, Weak<BuildProgress>>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Four equal workflow stages: source, contour generation, cache commit, and
/// read/decode/publication. Fractions are work units, not estimates of time left.
#[derive(Default)]
pub struct BuildProgress(AtomicU32);

impl BuildProgress {
    pub fn source_bytes(&self, done: u64, total: Option<u64>) {
        self.measured(0, 25, done, total);
    }

    pub fn source_ready(&self) {
        self.advance(25);
    }

    pub fn contours_ready(&self) {
        self.advance(50);
    }

    pub fn imported_rows(&self, done: u64, total: u64) {
        self.measured(50, 25, done, Some(total));
    }

    pub fn committed(&self) {
        self.advance(75);
    }

    fn measured(&self, start: u32, width: u32, done: u64, total: Option<u64>) {
        if let Some(total) = total.filter(|&total| total > 0) {
            // The final unit belongs to successful stage completion, not to
            // receiving an unvalidated body or inserting uncommitted rows.
            let units = ((done as f64 / total as f64) * width as f64) as u32;
            self.advance(start + units.min(width - 1));
        }
    }

    fn advance(&self, percent: u32) {
        self.0.fetch_max(percent, Ordering::Relaxed);
    }

    pub fn fraction(&self) -> f32 {
        self.0.load(Ordering::Relaxed) as f32 / 100.0
    }
}

pub fn start(path: &Path, zoom: i32, lat: i32, lon: i32) -> Arc<BuildProgress> {
    let progress = Arc::new(BuildProgress::default());
    if let Ok(mut registry) = registry().lock() {
        registry.retain(|_, entry| entry.strong_count() > 0);
        registry.insert((path.to_owned(), zoom, lat, lon), Arc::downgrade(&progress));
    }
    progress
}

/// Called by manifest workers so root discovery and registry traversal stay
/// off the paint thread. Snapshot handles subsequently need only atomic reads.
pub fn snapshot(path: &Path, zoom: i32) -> BuildSnapshot {
    registry()
        .lock()
        .map(|registry| {
            registry
                .iter()
                .filter_map(|((root, z, lat, lon), entry)| {
                    (root == path && *z == zoom)
                        .then(|| entry.upgrade().map(|p| ((*lat, *lon), p)))
                        .flatten()
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn clear() {
    if let Ok(mut registry) = registry().lock() {
        registry.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_measured_work_advances_and_completion_requires_success() {
        let p = BuildProgress::default();
        p.source_bytes(200, None);
        assert_eq!(p.fraction(), 0.0);
        p.source_bytes(50, Some(100));
        assert_eq!(p.fraction(), 0.12);
        p.source_bytes(10, Some(100));
        assert_eq!(p.fraction(), 0.12);
        p.source_bytes(100, Some(100));
        assert_eq!(p.fraction(), 0.24);
        p.source_ready();
        assert_eq!(p.fraction(), 0.25);
        p.contours_ready();
        p.imported_rows(99, 100);
        assert_eq!(p.fraction(), 0.74);
        p.imported_rows(100, 100);
        assert_eq!(p.fraction(), 0.74);
        p.committed();
        assert_eq!(p.fraction(), 0.75);
    }

    #[test]
    fn roots_zooms_retries_and_expired_workers_are_isolated() {
        let path = Path::new("progress-regression-earth.sqlite");
        let old = start(path, 10, 1, 2);
        old.committed();
        assert!(snapshot(Path::new("other-root.sqlite"), 10).is_empty());
        assert!(snapshot(path, 9).is_empty());
        let retry = start(path, 10, 1, 2);
        assert_eq!(snapshot(path, 10)[&(1, 2)].fraction(), 0.0);
        old.committed();
        assert_eq!(retry.fraction(), 0.0);
        drop(retry);
        assert!(snapshot(path, 10).is_empty());
    }
}
