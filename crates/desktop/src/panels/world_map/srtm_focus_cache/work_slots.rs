//! Separate bounded network work from CPU/disk terrain processing.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

pub struct Slots {
    active: AtomicUsize,
    limit: usize,
}

impl Slots {
    pub(super) const fn new(limit: usize) -> Self {
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
        Slots::new(processing_limit(cpus))
    })
}

fn processing_limit(cpus: usize) -> usize {
    configured_processing_limit(
        cpus,
        std::env::var("ONEKEE_TERRAIN_WORKERS").ok().as_deref(),
    )
}

fn configured_processing_limit(cpus: usize, value: Option<&str>) -> usize {
    // Eight workers won the full-view processing sweep on the 10-core host.
    // Keep two CPUs outside this budget for rendering and cached readers.
    let default = cpus.saturating_sub(2).clamp(1, 8);
    value
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| (1..=8).contains(value))
        .unwrap_or(default)
        .min(cpus.saturating_sub(1).max(1))
}

/// Admission reserves space before downloading. Downloaded rasters wait on
/// disk, not in memory; processing capacity is acquired separately.
pub struct DownloadQueue {
    downloads: Slots,
    staging: Slots,
}

impl DownloadQueue {
    const fn new(downloads: usize, staging: usize) -> Self {
        Self {
            downloads: Slots::new(downloads),
            staging: Slots::new(staging),
        }
    }

    pub fn try_acquire(&self) -> Option<DownloadPermit<'_>> {
        let staging = self.staging.try_acquire()?;
        let download = self.downloads.try_acquire()?;
        Some(DownloadPermit { download, staging })
    }
}

pub struct DownloadPermit<'a> {
    download: Permit<'a>,
    staging: Permit<'a>,
}

impl<'a> DownloadPermit<'a> {
    /// Release network capacity immediately; the returned permit bounds the
    /// downloaded queue until a processing worker takes ownership.
    pub fn downloaded(self) -> Permit<'a> {
        let Self { download, staging } = self;
        drop(download);
        staging
    }
}

pub fn remote_downloads() -> &'static DownloadQueue {
    static QUEUE: OnceLock<DownloadQueue> = OnceLock::new();
    QUEUE.get_or_init(|| {
        let downloads = download_limit(std::env::var("ONEKEE_TERRAIN_DOWNLOADS").ok().as_deref());
        DownloadQueue::new(downloads, downloads + 4)
    })
}

fn download_limit(value: Option<&str>) -> usize {
    value
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|v| (1..=25).contains(v))
        .unwrap_or(4)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn download_override_keeps_full_view_requests_bounded() {
        assert_eq!(download_limit(None), 4);
        assert_eq!(download_limit(Some("25")), 25);
        for invalid in ["0", "26", "many"] {
            assert_eq!(download_limit(Some(invalid)), 4);
        }
    }

    #[test]
    fn processing_budget_scales_and_overrides_cannot_remove_bounds() {
        for (cpus, expected) in [(1, 1), (2, 1), (4, 2), (8, 6), (10, 8), (64, 8)] {
            assert_eq!(configured_processing_limit(cpus, None), expected);
        }
        assert_eq!(configured_processing_limit(10, Some("6")), 6);
        assert_eq!(configured_processing_limit(4, Some("8")), 3);
        for invalid in ["0", "9", "-1", "many"] {
            assert_eq!(configured_processing_limit(10, Some(invalid)), 8);
        }
    }

    #[test]
    fn downloads_replenish_while_processing_is_busy_and_staging_stays_bounded() {
        let network = DownloadQueue::new(4, 8);
        let cpu = Slots::new(2);
        let downloads: Vec<_> = (0..4).map(|_| network.try_acquire().unwrap()).collect();
        assert!(network.try_acquire().is_none());
        let processing: Vec<_> = (0..2).map(|_| cpu.try_acquire().unwrap()).collect();
        assert!(cpu.try_acquire().is_none());
        assert!(cpu.acquire_until(|| true).is_none());
        let waiting: Vec<_> = downloads
            .into_iter()
            .map(DownloadPermit::downloaded)
            .collect();
        // The old whole-job limit stalled here. Four new downloads can now
        // start without either processing job finishing.
        let more: Vec<_> = (0..4).map(|_| network.try_acquire().unwrap()).collect();
        let more: Vec<_> = more.into_iter().map(DownloadPermit::downloaded).collect();
        assert!(network.try_acquire().is_none()); // all eight staging slots full
        drop(processing);
        let _processing = cpu.try_acquire().unwrap();
        drop(waiting); // handed to processing / cancelled
        assert!(network.try_acquire().is_some());
        drop(more);
        assert_eq!(network.staging.active.load(Ordering::Acquire), 0);
        assert_eq!(network.downloads.active.load(Ordering::Acquire), 0);
    }

    #[test]
    fn failed_downloads_and_cancelled_waiters_release_both_budgets() {
        let queue = DownloadQueue::new(1, 2);
        let download = queue.try_acquire().unwrap();
        for _ in 0..5 {
            assert!(queue.try_acquire().is_none());
        }
        assert_eq!(queue.staging.active.load(Ordering::Acquire), 1);
        drop(download);
        let waiting = queue.try_acquire().unwrap().downloaded();
        let _ = std::panic::catch_unwind(|| {
            let _download = queue.try_acquire().unwrap();
            panic!("download failed");
        });
        drop(waiting);
        assert_eq!(queue.staging.active.load(Ordering::Acquire), 0);
        assert_eq!(queue.downloads.active.load(Ordering::Acquire), 0);
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
