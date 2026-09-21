//! Separate bounded network work from CPU/disk terrain processing.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

pub struct Slots {
    active: AtomicUsize,
    limit: usize,
}

impl Slots {
    const fn new(limit: usize) -> Self {
        Self {
            active: AtomicUsize::new(0),
            limit,
        }
    }

    pub fn try_acquire(&self) -> Option<Permit<'_>> {
        self.active
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < self.limit).then_some(count + 1)
            })
            .ok()?;
        Some(Permit(self))
    }

    /// Background workers only. Shutdown can abandon a downloaded raster
    /// without waiting for another long-running GDAL process to complete.
    pub fn acquire_until(&self, cancelled: impl Fn() -> bool) -> Option<Permit<'_>> {
        while !cancelled() {
            if let Some(permit) = self.try_acquire() {
                return Some(permit);
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        None
    }
}

pub struct Permit<'a>(&'a Slots);

impl Drop for Permit<'_> {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::AcqRel);
    }
}

pub fn processing() -> &'static Slots {
    static SLOTS: OnceLock<Slots> = OnceLock::new();
    SLOTS.get_or_init(|| {
        let cpus = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        Slots::new(cpus.saturating_sub(1).clamp(1, 2))
    })
}

/// Covers download, waiting, and processing: at most four temporary rasters
/// and network workers exist, even when processing is the slower stage.
pub fn remote_jobs() -> &'static Slots {
    static SLOTS: Slots = Slots::new(4);
    &SLOTS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downloads_overlap_processing_without_exceeding_either_limit() {
        let network = Slots::new(4);
        let cpu = Slots::new(2);
        let downloads: Vec<_> = (0..4).map(|_| network.try_acquire().unwrap()).collect();
        assert!(network.try_acquire().is_none());
        let processing: Vec<_> = (0..2).map(|_| cpu.try_acquire().unwrap()).collect();
        assert!(cpu.try_acquire().is_none());
        assert!(cpu.acquire_until(|| true).is_none());
        drop(processing);
        assert!(cpu.try_acquire().is_some());
        assert!(network.try_acquire().is_none());
        drop(downloads);
        assert!(network.try_acquire().is_some());
    }

    #[test]
    fn concurrent_claims_never_exceed_the_limit_and_release_on_unwind() {
        let slots = Slots::new(2);
        std::thread::scope(|scope| {
            for _ in 0..8 {
                let slots = &slots;
                scope.spawn(move || {
                    for _ in 0..100 {
                        if let Some(_permit) = slots.try_acquire() {
                            assert!(slots.active.load(Ordering::Acquire) <= 2);
                            std::thread::yield_now();
                        }
                    }
                });
            }
        });
        let _ = std::panic::catch_unwind(|| {
            let _permit = slots.try_acquire().unwrap();
            panic!("simulated worker failure");
        });
        assert_eq!(slots.active.load(Ordering::Acquire), 0);
    }
}
