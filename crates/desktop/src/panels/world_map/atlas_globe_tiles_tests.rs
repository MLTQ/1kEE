use super::*;
use crate::model::GeoPoint;

fn fixture(key: Key) -> Arc<Raster> {
    let mut r = Raster::new(
        super::super::atlas_image::HeightGrid {
            width: 17,
            height: 17,
            metres: vec![1000.; 17 * 17],
        },
        key.region().bounds(),
    )
    .unwrap();
    Arc::get_mut(&mut r).unwrap().core = Some(key.bounds());
    r
}

#[test]
fn geographic_keys_wrap_and_share_sampled_edges() {
    assert_eq!(keys([179., -1., 181., 1.]), keys([-181., -1., -179., 1.]));
    let polar = keys([-180., 85., 180., 90.]);
    assert_eq!(polar.len(), 72);
    assert_eq!(polar.iter().collect::<HashSet<_>>().len(), 72);
    assert!(keys([0., 0., f32::NAN, 5.]).is_empty());
    let a = Key(12, 24);
    let b = Key(13, 24);
    assert_eq!(a.bounds()[2], b.bounds()[0]);
    assert_eq!(a.region().0[2] - b.region().0[0], 30);
    assert_eq!(a.region().size(15., 8192), [1202, 1202]);
}

#[test]
fn america_rockies_appalachians_camera_path_never_rebuilds_seen_tiles() {
    let ctx = egui::Context::default();
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280., 832.));
    let mut state = State::default();
    let builds = Arc::new(Mutex::new(HashMap::<Key, usize>::new()));
    let mut seen = HashMap::new();
    for (lat, lon, zoom) in [
        (39., -98., 3.),
        (40., -110., 32.),
        (38., -79., 38.),
        (39.01, -98.01, 3.01),
        (40.01, -110.01, 31.9),
        (38., -79., 38.),
        (39., -98., 3.),
    ] {
        let mut view = GlobeViewState::from_focus(GeoPoint { lat, lon });
        view.zoom = zoom;
        let wanted = selection(
            rect,
            &super::super::globe_scene::globe_layout(rect, &view),
            &view,
        );
        assert!(!wanted.is_empty());
        let began = Instant::now();
        loop {
            let builds = builds.clone();
            let frame = state.frame(
                &ctx,
                None,
                8192,
                &wanted,
                true,
                true,
                1,
                move |key, _, _| {
                    *builds.lock().unwrap().entry(key).or_default() += 1;
                    Ok(fixture(key))
                },
            );
            for key in &wanted {
                if let Some(entry) = state.entries.get(key)
                    && let Some(id) = seen.insert(*key, entry.raster.id)
                {
                    assert_eq!(id, entry.raster.id);
                }
            }
            if frame.missing == 0 {
                break;
            }
            assert!(began.elapsed() < Duration::from_secs(10));
            std::thread::sleep(Duration::from_millis(1));
        }
        eprintln!(
            "Atlas camera ({lat}, {lon}, {zoom}): {} visible, {} retained",
            wanted.len(),
            seen.len()
        );
    }
    assert!(
        seen.len() > 20,
        "exercise a real continent rather than three synthetic window IDs"
    );
    assert!(builds.lock().unwrap().values().all(|&count| count == 1));
    // Far zoom does not replace ready terrain with the overview.
    let frame = state.frame(
        &ctx,
        None,
        8192,
        &[Key(12, 24)],
        false,
        true,
        1,
        |_, _, _| panic!("no load"),
    );
    assert_eq!(frame.missing, 0);
    assert_eq!(frame.rasters.len(), 1);
}

#[test]
fn busy_worker_does_not_hide_warm_tiles_and_clear_rejects_old_results() {
    let ctx = egui::Context::default();
    let mut state = State::default();
    let a = Key(12, 24);
    let b = Key(20, 25);
    state.identity = Some((None, 1202));
    let raster = fixture(a);
    state.bytes = raster.heights.len() * 8;
    state.entries.insert(
        a,
        Entry {
            raster: raster.clone(),
            touched: 0,
        },
    );
    let (sender, result) = mpsc::channel();
    state.job = Some(Job {
        key: b,
        epoch: state.epoch,
        result,
    });
    let frame = state.frame(&ctx, None, 8192, &[a, b], true, true, 1, |_, _, _| {
        panic!("already busy")
    });
    assert_eq!(frame.rasters[0].id, raster.id);
    assert_eq!(frame.missing, 1);
    state.clear();
    sender.send(Ok(fixture(b))).unwrap();
    state.poll();
    assert!(state.entries.is_empty());
    assert!(state.job.is_none());
}

#[test]
fn failed_tiles_retry_with_backoff_and_source_changes_discard_results() {
    let ctx = egui::Context::default();
    let mut state = State {
        identity: Some((None, 1202)),
        ..Default::default()
    };
    let key = Key(10, 20);
    state
        .failed
        .insert(key, (Instant::now(), "unplugged drive".into()));
    let frame = state.frame(&ctx, None, 8192, &[key], true, false, 1, |_, _, _| {
        panic!("retry storm")
    });
    assert!(frame.error.unwrap().contains("unplugged"));
    assert!(state.job.is_none());
    let (sender, result) = mpsc::channel();
    state.job = Some(Job {
        key,
        epoch: state.epoch,
        result,
    });
    sender.send(Ok(fixture(key))).unwrap();
    state.frame(
        &ctx,
        Some(Path::new("different-root")),
        8192,
        &[key],
        false,
        false,
        1,
        |_, _, _| unreachable!(),
    );
    assert!(state.entries.is_empty());
}

#[test]
#[ignore = "reads native GEBCO, writes only a private cache; verifies persistent canonical tile reuse"]
fn native_globe_tiles_persist_lossless_prepared_terrain() {
    let source = PathBuf::from(std::env::var("ONEKEE_ATLAS_ROOT").expect("terrain root"));
    let scratch = tempfile::tempdir().unwrap();
    std::fs::create_dir(scratch.path().join("Derived")).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        crate::terrain_assets::find_data_root(Some(&source)).unwrap(),
        scratch.path().join("Data"),
    )
    .unwrap();
    let ctx = egui::Context::default();
    let mut state = State::default();
    let wanted = [Key(14, 25), Key(20, 25)]; // Rockies and Appalachians, 35–40°N.
    let started = Instant::now();
    let first = loop {
        let frame = state.frame(
            &ctx,
            Some(scratch.path()),
            8192,
            &wanted,
            true,
            true,
            1,
            |key, root, edge| {
                super::super::atlas_source::load_globe(root.as_deref(), key.region(), edge)?
                    .into_raster_with_core(Some(key.bounds()))
                    .ok_or("bad tile".into())
            },
        );
        assert!(frame.error.is_none(), "{:?}", frame.error);
        if frame.missing == 0 {
            break frame.rasters;
        }
        assert!(started.elapsed() < Duration::from_secs(180));
        std::thread::sleep(Duration::from_millis(10));
    };
    eprintln!(
        "Native Globe tiles: cold {:.1} ms",
        started.elapsed().as_secs_f64() * 1000.
    );
    let started = Instant::now();
    for key in [wanted[1], wanted[0], wanted[1]] {
        let frame = state.frame(
            &ctx,
            Some(scratch.path()),
            8192,
            &[key],
            true,
            true,
            1,
            |_, _, _| panic!("warm tile rebuilt"),
        );
        assert_eq!(frame.missing, 0);
        assert!(first.iter().any(|r| Arc::ptr_eq(r, &frame.rasters[0])));
    }
    eprintln!(
        "Native Globe tiles: three warm revisits {:.3} ms",
        started.elapsed().as_secs_f64() * 1000.
    );
    super::super::atlas_source::flush_view_cache();
    // Verify both prepared materials and exact measured heights on reload.
    let db = scratch
        .path()
        .join("Derived/terrain/atlas_recent_views_v1.sqlite");
    let c = rusqlite::Connection::open(db).unwrap();
    let count: usize = c
        .query_row("SELECT count(*) FROM materials", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 2);
    for key in wanted {
        let started = Instant::now();
        let detail =
            super::super::atlas_source::load_globe(Some(scratch.path()), key.region(), 1202)
                .unwrap();
        let raster = detail.into_raster_with_core(Some(key.bounds())).unwrap();
        let original = first.iter().find(|r| r.core == raster.core).unwrap();
        assert_eq!(raster.colors, original.colors);
        assert!(
            raster
                .heights
                .iter()
                .zip(&original.heights)
                .all(|(a, b)| a.to_bits() == b.to_bits())
        );
        eprintln!(
            "Native Globe tile {:?}: disk {:.1} ms",
            key,
            started.elapsed().as_secs_f64() * 1000.
        );
    }
}
