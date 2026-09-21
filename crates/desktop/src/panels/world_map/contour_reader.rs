//! One coordinator per local cache, with at most two streaming SQLite readers.
use super::*;

pub(super) fn publish_tile(
    cache: &Mutex<LocalRegionCache>,
    epoch: u64,
    key: CacheKey,
    contours: Vec<ContourPath>,
) -> bool {
    let Ok(mut cache) = cache.lock() else {
        return false;
    };
    if cache.load_epoch != epoch || cache.load_in_flight != Some(epoch) {
        return false;
    }
    cache.in_flight.remove(&key);
    cache.read_progress.remove(&key);
    if let std::collections::hash_map::Entry::Vacant(entry) = cache.entries.entry(key) {
        entry.insert(Arc::new(contours));
        cache.mark_entries_changed();
    }
    true
}

pub(super) fn spawn_local_read(
    cache: &'static Mutex<LocalRegionCache>,
    epoch: u64,
    requests: Vec<ContourReadRequest>,
    feature_budget: usize,
    ctx: egui::Context,
    worker_name: &'static str,
) {
    let cleanup_requests = requests.clone();
    let cleanup_ctx = ctx.clone();
    if let Err(error) = std::thread::Builder::new()
        .name(worker_name.into())
        .spawn(move || {
            let success = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let workers = std::thread::available_parallelism()
                    .map(|n| if n.get() >= 4 { 2 } else { 1 })
                    .unwrap_or(1)
                    .min(requests.len());
                std::thread::scope(|scope| {
                    let handles: Vec<_> = (0..workers)
                        .map(|worker| {
                            // Interleaving starts with the two closest tiles. Each
                            // reader reuses one connection and publishes per tile.
                            let work: Vec<_> = requests
                                .iter()
                                .skip(worker)
                                .step_by(workers)
                                .cloned()
                                .collect();
                            let ctx = &ctx;
                            scope.spawn(move || {
                                stream_local_contours(
                                    &work[0].0.path,
                                    &work,
                                    feature_budget,
                                    &mut |key, done, total| {
                                        if let Ok(mut guard) = cache.lock()
                                            && guard.load_epoch == epoch
                                            && guard.load_in_flight == Some(epoch)
                                            && total > 0
                                        {
                                            guard
                                                .read_progress
                                                .insert(key.clone(), done as f32 / total as f32);
                                        }
                                    },
                                    &mut |key, contours| {
                                        let current = publish_tile(cache, epoch, key, contours);
                                        if current {
                                            ctx.request_repaint();
                                        }
                                        current // abandon the remaining batch after a reset
                                    },
                                )
                                .is_ok()
                            })
                        })
                        .collect();
                    // Join every worker even if an earlier one succeeded.
                    let mut any = false;
                    for handle in handles {
                        any |= handle.join().unwrap();
                    }
                    any
                })
            }))
            .unwrap_or_else(|_| {
                eprintln!("[1kEE] {worker_name} panicked while reading contour tiles");
                false
            });
            // Tiles are already published; finish only the batch bookkeeping.
            finish_local_read(cache, epoch, &requests, success.then(Vec::new));
            if success {
                ctx.request_repaint();
            } else {
                ctx.request_repaint_after(CONTOUR_READ_RETRY_DELAY);
            }
        })
    {
        finish_local_read(cache, epoch, &cleanup_requests, None);
        eprintln!("[1kEE] failed to spawn {worker_name}: {error}");
        cleanup_ctx.request_repaint_after(CONTOUR_READ_RETRY_DELAY);
    }
}
