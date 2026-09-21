//! A paint-safe loading snapshot from build handles and decoded/published tiles.
use super::*;

pub(super) struct Snapshot {
    pub ready_buckets: HashSet<(i32, i32)>,
    pub fractions: HashMap<(i32, i32), f32>,
    pub status: srtm_focus_cache::FocusContourRegionStatus,
}

pub(super) fn snapshot(
    cache: &Mutex<LocalRegionCache>,
    assets: &LocalManifestAssetViews,
    state: srtm_focus_cache::LocalContourRegionState,
) -> Snapshot {
    let mut ready = state.ready_buckets;
    let mut status = state.status;
    let mut fractions: HashMap<_, _> = assets
        .build_progress
        .iter()
        .map(|(&bucket, progress)| (bucket, progress.fraction()))
        .collect();
    let mut guard = cache.lock().ok();
    for asset in assets.display_assets() {
        let bucket = (asset.lat_bucket, asset.lon_bucket);
        // Ignore assets outside the currently counted build window.
        if !ready.contains(&bucket) {
            continue;
        }
        let key = CacheKey {
            path: asset.path.clone(),
            zoom_bucket: asset.zoom_bucket,
            lat_bucket: asset.lat_bucket,
            lon_bucket: asset.lon_bucket,
        };
        let fraction = guard
            .as_ref()
            .map(|cache| {
                if (cache.entries.contains_key(&key) && cache.published_tiles.contains(&key))
                    || cache
                        .entries
                        .get(&key)
                        .is_some_and(|lines| lines.is_empty())
                {
                    1.0
                } else if cache.entries.contains_key(&key) {
                    0.99 // decoded, awaiting publication of the display merge
                } else {
                    0.75 + 0.24
                        * cache
                            .read_progress
                            .get(&key)
                            .copied()
                            .unwrap_or(0.0)
                            .clamp(0.0, 1.0)
                }
            })
            .unwrap_or(0.75);
        fractions.insert(bucket, fraction);
        if fraction < 1.0 {
            ready.remove(&bucket);
            status.ready_assets = status.ready_assets.saturating_sub(1);
        }
    }
    // No-source tiles remain complete, even if an old build handle survives.
    for &bucket in &ready {
        fractions.insert(bucket, 1.0);
    }
    if let Some(cache) = guard.as_mut() {
        cache.last_status = Some(status);
    }
    Snapshot {
        ready_buckets: ready,
        fractions,
        status,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disk_cache_is_not_complete_until_decode_and_publication() {
        let key = CacheKey {
            path: "loading-test.sqlite".into(),
            zoom_bucket: 10,
            lat_bucket: 0,
            lon_bucket: 0,
        };
        let assets = LocalManifestAssetViews {
            assets: vec![srtm_focus_cache::FocusContourAsset {
                path: key.path.clone(),
                zoom_bucket: 10,
                lat_bucket: 0,
                lon_bucket: 0,
                simplify_step: 1,
            }],
            ..Default::default()
        };
        let cache = Mutex::new(LocalRegionCache::default());
        let read = || {
            snapshot(
                &cache,
                &assets,
                srtm_focus_cache::LocalContourRegionState {
                    ready_buckets: HashSet::from([(0, 0), (1, 1)]),
                    status: srtm_focus_cache::FocusContourRegionStatus {
                        ready_assets: 1,
                        pending_assets: 0,
                        total_assets: 1,
                    },
                },
            )
        };
        assert_eq!(read().fractions[&(0, 0)], 0.75);
        assert_eq!(read().fractions[&(1, 1)], 1.0); // known no-source tile
        cache.lock().unwrap().read_progress.insert(key.clone(), 0.5);
        assert_eq!(read().fractions[&(0, 0)], 0.87);
        cache.lock().unwrap().entries.insert(
            key.clone(),
            Arc::new(vec![ContourPath {
                elevation_m: 1.0,
                points: vec![GeoPoint { lat: 0.0, lon: 0.0 }],
            }]),
        );
        assert_eq!(read().fractions[&(0, 0)], 0.99);
        assert_eq!(read().status.ready_assets, 0);
        cache.lock().unwrap().published_tiles.insert(key.clone());
        assert_eq!(read().fractions[&(0, 0)], 1.0);
        assert_eq!(read().status.ready_assets, 1);
        cache.lock().unwrap().clear_entries_for_new_scene();
        assert_eq!(read().fractions[&(0, 0)], 0.75);
    }
}
