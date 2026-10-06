//! Coalesced off-thread Earth globe manifest selection and build scheduling.
use super::*;

#[derive(Default)]
pub(super) struct State {
    requested: Option<LocalManifestKey>,
    snapshot: Option<(
        LocalManifestKey,
        Vec<srtm_focus_cache::FocusContourAsset>,
        u64,
        Instant,
    )>,
    busy: bool,
    epoch: u64,
}

impl State {
    pub fn invalidate(&mut self) {
        self.requested = None;
        self.snapshot = None;
        self.epoch = self.epoch.wrapping_add(1);
        // Keep the single-flight gate occupied until the old worker returns.
    }

    fn finish(
        &mut self,
        epoch: u64,
        key: LocalManifestKey,
        revision: u64,
        result: Option<Vec<srtm_focus_cache::FocusContourAsset>>,
    ) {
        self.busy = false;
        if epoch == self.epoch
            && self.requested.as_ref() == Some(&key)
            && let Some(assets) = result
        {
            self.snapshot = Some((key, assets, revision, Instant::now()));
        }
    }
}

pub(super) fn assets(
    cache: &'static Mutex<GlobeRegionCache>,
    root: Option<&Path>,
    center: GeoPoint,
    zoom: f32,
    radius: i32,
    ctx: &egui::Context,
) -> Vec<srtm_focus_cache::FocusContourAsset> {
    let step = srtm_focus_cache::half_extent_for_zoom(zoom) * 0.45;
    let key = LocalManifestKey {
        root: root.map(Path::to_path_buf),
        center_lat_bucket: (center.lat / step).round() as i32,
        center_lon_bucket: (center.lon / step).round() as i32,
        zoom_bucket: srtm_focus_cache::zoom_bucket_for_zoom(zoom),
        prefetch_radius: radius,
        build_radius: radius,
    };
    let revision = srtm_focus_cache::contour_manifest_revision();
    let mut guard = cache.lock().unwrap();
    let state = &mut guard.manifest;
    state.requested = Some(key.clone());
    let snapshot = state.snapshot.as_ref().filter(|s| s.0 == key);
    let result = snapshot.map_or_else(Vec::new, |s| s.1.clone());
    let fresh =
        snapshot.is_some_and(|s| s.2 == revision && s.3.elapsed() < LOCAL_MANIFEST_SNAPSHOT_TTL);
    if !fresh && !state.busy {
        state.busy = true;
        let epoch = state.epoch;
        let wake = ctx.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("earth-globe-manifest".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    srtm_focus_cache::ensure_focus_contour_region(
                        key.root.as_deref(),
                        center,
                        zoom,
                        radius,
                        radius,
                    )
                }))
                .ok();
                cache
                    .lock()
                    .unwrap()
                    .manifest
                    .finish(epoch, key, revision, result);
                wake.request_repaint();
            })
        {
            state.busy = false;
            eprintln!("[1kEE] globe manifest worker: {error}");
        }
    }
    ctx.request_repaint_after(LOCAL_MANIFEST_SNAPSHOT_TTL);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    fn key(x: i32) -> LocalManifestKey {
        LocalManifestKey {
            root: None,
            center_lat_bucket: 0,
            center_lon_bucket: x,
            zoom_bucket: 0,
            prefetch_radius: 5,
            build_radius: 5,
        }
    }
    #[test]
    fn late_manifest_cannot_restore_a_reset_or_panned_scene() {
        let mut state = State {
            requested: Some(key(0)),
            busy: true,
            ..Default::default()
        };
        state.invalidate();
        assert!(state.busy);
        state.requested = Some(key(0));
        state.finish(0, key(0), 0, Some(vec![]));
        assert!(state.snapshot.is_none());
        assert!(!state.busy);
        state.requested = Some(key(1));
        state.finish(1, key(0), 0, Some(vec![]));
        assert!(state.snapshot.is_none());
        state.finish(1, key(1), 42, Some(vec![]));
        assert_eq!(state.snapshot.as_ref().unwrap().2, 42);
    }
}
