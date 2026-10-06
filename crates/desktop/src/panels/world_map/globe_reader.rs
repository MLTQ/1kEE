//! Publish native globe tiles as they decode, not after the entire region.
use super::*;

fn publish(
    cache: &Mutex<GlobeRegionCache>,
    epoch: u64,
    key: CacheKey,
    contours: Vec<ContourPath>,
) -> bool {
    let bounds = residency::Bounds::from_contours(&contours);
    let cost = globe_residency::Cost::measure(&contours);
    let mut guard = cache.lock().unwrap();
    if guard.load_epoch != epoch || guard.load_in_flight != Some(epoch) {
        return false;
    }
    let cell = (key.lat_bucket, key.lon_bucket);
    guard.in_flight.remove(&cell);
    guard.residency.record(cell, bounds);
    if guard.residency.wanted(cell) && !guard.tiles.contains_key(&cell) {
        guard.residency.admit(cell, cost);
        guard.tiles.insert(cell, Arc::new(contours));
        guard.order.push(cell);
        guard.mark_tiles_changed();
    }
    true
}

pub(super) fn spawn(
    cache: &'static Mutex<GlobeRegionCache>,
    epoch: u64,
    requests: Vec<ContourReadRequest>,
    ctx: egui::Context,
) {
    let cleanup = requests.clone();
    let wake = ctx.clone();
    if let Err(error) = std::thread::Builder::new()
        .name("earth-globe-contour-read".into())
        .spawn(move || {
            let success = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                stream_local_contours(
                    &requests[0].0.path,
                    &requests,
                    ReadSelection::WholeTile(usize::MAX),
                    &mut |_, _, _| {},
                    &mut |key, contours| {
                        let current = publish(cache, epoch, key, contours);
                        if current {
                            ctx.request_repaint();
                        }
                        current
                    },
                )
                .is_ok()
            }))
            .unwrap_or(false);
            finish_globe_read(cache, epoch, &requests, success.then(Vec::new));
            ctx.request_repaint_after(if success {
                Duration::ZERO
            } else {
                CONTOUR_READ_RETRY_DELAY
            });
        })
    {
        finish_globe_read(cache, epoch, &cleanup, None);
        wake.request_repaint_after(CONTOUR_READ_RETRY_DELAY);
        eprintln!("[1kEE] globe tile reader: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn first_tile_publishes_before_batch_completion_and_reset_cancels_remaining_tiles() {
        let cache = Mutex::new(GlobeRegionCache {
            load_in_flight: Some(0),
            ..Default::default()
        });
        let key = CacheKey {
            path: PathBuf::new(),
            zoom_bucket: 0,
            lat_bucket: 9,
            lon_bucket: 27,
        };
        assert!(publish(&cache, 0, key.clone(), vec![]));
        assert!(cache.lock().unwrap().tiles.contains_key(&(9, 27)));
        assert_eq!(cache.lock().unwrap().load_in_flight, Some(0));
        cache.lock().unwrap().reset_all();
        assert!(!publish(&cache, 0, key, vec![]));
        assert!(cache.lock().unwrap().tiles.is_empty());
    }
}
