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
    loading: Option<Loading>,
}
pub(super) struct Loading {
    pub key: LocalManifestKey,
    pub state: srtm_focus_cache::LocalContourRegionState,
    pub builds: BuildSnapshot,
    pub available: bool,
    pub assets: HashSet<(i32, i32)>,
}

impl State {
    pub fn invalidate(&mut self) {
        self.requested = None;
        self.snapshot = None;
        self.loading = None;
        self.epoch = self.epoch.wrapping_add(1);
        // Keep the single-flight gate occupied until the old worker returns.
    }

    fn finish(
        &mut self,
        epoch: u64,
        key: LocalManifestKey,
        revision: u64,
        result: Option<Vec<srtm_focus_cache::FocusContourAsset>>,
        loading: Option<Loading>,
    ) {
        self.busy = false;
        if epoch == self.epoch
            && self.requested.as_ref() == Some(&key)
            && let Some(assets) = result
        {
            self.snapshot = Some((key, assets, revision, Instant::now()));
            self.loading = loading;
        }
    }
    pub fn loading(&self) -> Option<&Loading> {
        self.loading
            .as_ref()
            .filter(|p| Some(&p.key) == self.requested.as_ref())
    }
    pub fn selecting(&self) -> bool {
        self.requested.is_some() && self.busy && self.loading().is_none()
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
    assets_for(
        cache,
        root,
        center,
        zoom,
        radius,
        crate::model::ActiveBody::Earth,
        ctx,
    )
}

pub(super) fn assets_for(
    cache: &'static Mutex<GlobeRegionCache>,
    root: Option<&Path>,
    center: GeoPoint,
    zoom: f32,
    radius: i32,
    body: crate::model::ActiveBody,
    ctx: &egui::Context,
) -> Vec<srtm_focus_cache::FocusContourAsset> {
    use crate::model::ActiveBody;
    let spec = match body {
        ActiveBody::Earth => srtm_focus_cache::zoom::spec_for_zoom(zoom),
        ActiveBody::Moon => srtm_focus_cache::zoom::lunar_spec_for_zoom(zoom),
        ActiveBody::Mars => srtm_focus_cache::zoom::mars_spec_for_zoom(zoom),
    };
    let step = spec.half_extent_deg * 0.45;
    let key = LocalManifestKey {
        build_at: BuildAt::Viewport,
        root: root.map(Path::to_path_buf),
        center_lat_bucket: (center.lat / step).round() as i32,
        center_lon_bucket: (center.lon / step).round() as i32,
        zoom_bucket: spec.zoom_bucket,
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
            .name("globe-manifest".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let load = match body {
                        ActiveBody::Earth => srtm_focus_cache::ensure_focus_contour_region,
                        ActiveBody::Moon => srtm_focus_cache::ensure_lunar_contour_region,
                        ActiveBody::Mars => srtm_focus_cache::ensure_mars_contour_region,
                    };
                    let assets = load(key.root.as_deref(), center, zoom, radius, radius);
                    let loading = (body == ActiveBody::Earth).then(|| Loading {
                        key: key.clone(),
                        state: srtm_focus_cache::local_contour_region_state(
                            body, &assets, center, zoom, radius,
                        ),
                        builds: srtm_focus_cache::db::focus_cache_db_path_for_zoom(
                            key.root.as_deref(),
                            zoom,
                        )
                        .map(|path| srtm_focus_cache::progress::snapshot(&path, key.zoom_bucket))
                        .unwrap_or_default(),
                        available: terrain_assets::find_srtm_root(key.root.as_deref()).is_some(),
                        assets: assets
                            .iter()
                            .map(|a| (a.lat_bucket, a.lon_bucket))
                            .collect(),
                    });
                    (assets, loading)
                }))
                .ok();
                let (result, loading) =
                    result.map_or((None, None), |(assets, loading)| (Some(assets), loading));
                cache
                    .lock()
                    .unwrap()
                    .manifest
                    .finish(epoch, key, revision, result, loading);
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
            build_at: BuildAt::Viewport,
            root: None,
            center_lat_bucket: 0,
            center_lon_bucket: x,
            zoom_bucket: 0,
            prefetch_radius: 5,
            build_radius: 5,
        }
    }
    #[test]
    fn in_flight_globe_selection_keeps_repainting_without_a_database_poll() {
        // Simulate a worker stuck in filesystem/SQLite I/O. The UI can still
        // draw and schedule retries using only the coordinator's memory.
        let cache: &'static Mutex<GlobeRegionCache> =
            Box::leak(Box::new(Mutex::new(GlobeRegionCache::default())));
        cache.lock().unwrap().manifest.busy = true;
        let ctx = egui::Context::default();
        for _ in 0..4 {
            let output = ctx.run(egui::RawInput::default(), |ctx| {
                assert!(
                    assets(
                        cache,
                        Some(Path::new("/unavailable-terrain-source")),
                        GeoPoint {
                            lat: 40.0,
                            lon: -120.0
                        },
                        1.5,
                        5,
                        ctx,
                    )
                    .is_empty()
                );
            });
            assert!(
                output.viewport_output[&egui::ViewportId::ROOT].repaint_delay
                    <= LOCAL_MANIFEST_SNAPSHOT_TTL
            );
            assert!(cache.lock().unwrap().manifest.busy);
        }
        // Even a completed empty snapshot keeps the external-builder refresh
        // alive; stopping the old panel poll must not strand a stationary view.
        let mut guard = cache.lock().unwrap();
        let state = &mut guard.manifest;
        state.finish(
            state.epoch,
            state.requested.clone().unwrap(),
            srtm_focus_cache::contour_manifest_revision(),
            Some(vec![]),
            None,
        );
        drop(guard);
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            assert!(
                assets(
                    cache,
                    Some(Path::new("/unavailable-terrain-source")),
                    GeoPoint {
                        lat: 40.0,
                        lon: -120.0
                    },
                    1.5,
                    5,
                    ctx,
                )
                .is_empty()
            );
        });
        assert!(
            output.viewport_output[&egui::ViewportId::ROOT].repaint_delay
                <= LOCAL_MANIFEST_SNAPSHOT_TTL
        );
        assert!(!cache.lock().unwrap().manifest.busy);
    }

    #[test]
    fn globe_loading_tracks_source_decode_publication_and_rejects_old_viewports() {
        let mut key = key(0);
        key.build_radius = 0;
        let info = Loading {
            key: key.clone(),
            state: srtm_focus_cache::LocalContourRegionState {
                ready_buckets: HashSet::from([(0, 0)]),
                status: srtm_focus_cache::FocusContourRegionStatus {
                    ready_assets: 1,
                    pending_assets: 0,
                    total_assets: 1,
                },
            },
            builds: BuildSnapshot::new(),
            available: true,
            assets: HashSet::from([(0, 0)]),
        };
        let mut cache = GlobeRegionCache {
            manifest: State {
                requested: Some(key.clone()),
                loading: Some(info),
                ..Default::default()
            },
            ..Default::default()
        };
        let snapshot = super::super::globe_loading::snapshot;
        assert_eq!(snapshot(&cache).reading, 1);
        assert_eq!(snapshot(&cache).fraction(), 0.75);
        cache.read_progress.insert((0, 0), 0.5);
        assert_eq!(snapshot(&cache).fraction(), 0.87);
        cache.tiles.insert(
            (0, 0),
            Arc::new(vec![ContourPath {
                elevation_m: 1.,
                points: vec![GeoPoint { lat: 0., lon: 0. }],
            }]),
        );
        assert_eq!(snapshot(&cache).preparing, 1);
        assert_eq!(snapshot(&cache).ready, 0);
        cache.merged_revision = Some(cache.tiles_revision);
        assert!(!snapshot(&cache).active());
        cache.manifest.busy = true;
        assert!(
            !snapshot(&cache).active(),
            "periodic manifest refresh is not a new load"
        );
        cache.manifest.requested.as_mut().unwrap().center_lon_bucket = 3;
        assert_eq!(snapshot(&cache).selecting, 1);
        assert_eq!(snapshot(&cache).total, 0);
        cache.reset_all();
        assert!(!snapshot(&cache).active());
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
        state.finish(0, key(0), 0, Some(vec![]), None);
        assert!(state.snapshot.is_none());
        assert!(!state.busy);
        state.requested = Some(key(1));
        state.finish(1, key(0), 0, Some(vec![]), None);
        assert!(state.snapshot.is_none());
        state.finish(1, key(1), 42, Some(vec![]), None);
        assert_eq!(state.snapshot.as_ref().unwrap().2, 42);
    }
}
