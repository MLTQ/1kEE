//! Regression for mode switches retaining actual uploaded Atlas textures.
use super::*;
fn block_on<T>(future: impl std::future::Future<Output = T>) -> T {
    use std::task::{Context, Poll, Wake, Waker};
    struct Unpark(std::thread::Thread);
    impl Wake for Unpark {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = Waker::from(Arc::new(Unpark(std::thread::current())));
    let mut context = Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(value) => return value,
            Poll::Pending => std::thread::park_timeout(std::time::Duration::from_millis(10)),
        }
    }
}
fn gpu() -> (wgpu::Device, wgpu::Queue) {
    let instance = wgpu::Instance::default();
    let adapter = if let Some(name) = crate::gpu_config::adapter_name() {
        crate::gpu_config::select_adapter(
            &instance.enumerate_adapters(wgpu::Backends::all()),
            None,
            &name,
        )
        .unwrap()
    } else {
        block_on(instance.request_adapter(&Default::default())).unwrap()
    };
    eprintln!("Atlas retention adapter: {:?}", adapter.get_info());
    block_on(adapter.request_device(&Default::default(), None)).unwrap()
}
#[test]
#[ignore = "requires GPU; verifies zero re-upload after switching Atlas modes and explicit invalidation"]
fn atlas_gpu_revisit_reuses_uploaded_sources_and_render_targets() {
    let (device, queue) = gpu();
    let mut res = Resources::new(&device);
    let ctx = egui::Context::default();
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(128., 128.));
    let painter = ctx
        .layer_painter(egui::LayerId::background())
        .with_clip_rect(rect);
    let focus = GeoPoint { lat: 46., lon: 7.5 };
    let view = GlobeViewState::from_focus(focus);
    let layout = LocalLayout {
        center: rect.center(),
        focus_center: rect.center(),
        width: 128.,
        height: 128.,
        horizontal_scale: 40.,
    };
    let data = Raster::new(
        HeightGrid {
            width: 129,
            height: 129,
            metres: vec![1000.; 129 * 129],
        },
        [7., 45.5, 8., 46.5],
    )
    .unwrap();
    let mut callback = Callback::local(&painter, &layout, &view, data, focus, 0.5, true, false, 1.);
    let prepare = |res: &mut Resources, cb: &Callback| {
        let mut encoder = device.create_command_encoder(&Default::default());
        res.prepare(&device, &queue, &mut encoder, cb, 1.);
        queue.submit([encoder.finish()]);
        device.poll(wgpu::Maintain::Wait);
    };
    prepare(&mut res, &callback);
    let first_bytes = res.uploaded_bytes;
    assert_eq!(first_bytes, 129 * 129 * 8);
    // Both real callback slots are exercised. The projection is irrelevant to
    // residency, and keeping identical uniforms also checks render-target reuse.
    callback.slot = 0;
    prepare(&mut res, &callback);
    assert_eq!(res.uploaded_bytes, first_bytes * 2);
    let keys: Vec<_> = res.slots.iter().map(|slot| slot.key.clone()).collect();
    let generation = res.generation;
    for slot in [1, 0, 1, 0] {
        res.sync_generation(generation); // also called while Atlas is hidden
        callback.slot = slot;
        prepare(&mut res, &callback);
        assert_eq!(
            res.uploaded_bytes,
            first_bytes * 2,
            "mode revisit must upload zero bytes"
        );
        assert_eq!(res.slots[slot].key, keys[slot]);
        assert!(res.slots[slot].target.is_some());
    }
    res.sync_generation(generation + 1);
    assert!(
        res.slots
            .iter()
            .all(|slot| slot.sources.is_empty() && slot.target.is_none())
    );
    prepare(&mut res, &callback);
    assert_eq!(
        res.uploaded_bytes,
        first_bytes * 3,
        "explicit invalidation forces a new upload"
    );
}

#[test]
#[ignore = "requires GPU; A-B-C-A Globe revisits without upload, Local eviction and incomplete-source cleanup"]
fn atlas_gpu_globe_regions_stay_resident_but_local_still_evicts() {
    use crate::panels::world_map::globe_scene::globe_layout;
    let (device, queue) = gpu();
    let ctx = egui::Context::default();
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(256., 256.));
    let painter = ctx
        .layer_painter(egui::LayerId::background())
        .with_clip_rect(rect);
    let raster = |size: usize, bounds| {
        Raster::new(
            HeightGrid {
                width: size,
                height: size,
                metres: vec![1000.; size * size],
            },
            bounds,
        )
        .unwrap()
    };
    let overview = raster(129, [-180., -90., 180., 90.]);
    let regions: Vec<_> = [7., 19., -75.]
        .iter()
        .map(|&lon| raster(129, [lon - 5., 40., lon + 5., 50.]))
        .collect();
    let make_globe = |data: Arc<Raster>| {
        let view = GlobeViewState::from_focus(GeoPoint {
            lat: 45.,
            lon: (data.bounds[0] + data.bounds[2]) * 0.5,
        });
        Callback::globe(
            &painter,
            rect,
            &globe_layout(rect, &view),
            &view,
            vec![overview.clone(), data],
            true,
            true,
            1.,
        )
    };
    let prepare = |res: &mut Resources, callback: &Callback| {
        let mut encoder = device.create_command_encoder(&Default::default());
        res.prepare(&device, &queue, &mut encoder, callback, 1.);
        queue.submit([encoder.finish()]);
        device.poll(wgpu::Maintain::Wait);
    };
    let bytes_per_region = 129 * 129 * 8;
    for keep in [false, true] {
        let mut res = Resources::new(&device);
        res.keep_globe = keep;
        for region in &regions {
            prepare(&mut res, &make_globe(region.clone()));
        }
        assert_eq!(res.slots[0].sources.len(), if keep { 4 } else { 2 });
        let before = res.uploaded_bytes;
        prepare(&mut res, &make_globe(regions[0].clone()));
        let revisit_bytes = res.uploaded_bytes - before;
        assert_eq!(revisit_bytes, if keep { 0 } else { bytes_per_region });
        eprintln!("Atlas Globe keep={keep}: A-B-C-A revisit uploaded {revisit_bytes} bytes");

        let focus = GeoPoint { lat: 45., lon: 7. };
        let view = GlobeViewState::from_focus(focus);
        let layout = LocalLayout {
            center: rect.center(),
            focus_center: rect.center(),
            width: 256.,
            height: 256.,
            horizontal_scale: 80.,
        };
        for index in [0, 1, 0] {
            let cb = Callback::local(
                &painter,
                &layout,
                &view,
                regions[index].clone(),
                focus,
                5.,
                true,
                false,
                1.,
            );
            let before = res.uploaded_bytes;
            prepare(&mut res, &cb);
            assert_eq!(
                res.uploaded_bytes - before,
                bytes_per_region,
                "Local still retires replaced sources"
            );
            assert_eq!(res.slots[1].sources.len(), 1);
        }
        // Abandoned partial GPU uploads are not completed tiles and must not
        // accumulate when the user navigates before staging finishes.
        let partial = raster(2048, [0., 40., 10., 50.]);
        prepare(&mut res, &make_globe(partial.clone()));
        assert!(res.slots[0].sources[&partial.id].row < partial.size[1]);
        prepare(&mut res, &make_globe(regions[0].clone()));
        assert!(!res.slots[0].sources.contains_key(&partial.id));
        res.sync_generation(res.generation + 1);
        assert!(res.slots.iter().all(|slot| slot.sources.is_empty()));
    }
}

#[test]
#[ignore = "requires GPU; actual America/Rockies/Appalachians camera selection and incremental tile publication"]
fn atlas_gpu_camera_tiles_revisit_without_build_or_upload() {
    use crate::panels::world_map::{
        atlas_globe_tiles::{self as tiles, Key, State},
        globe_scene::globe_layout,
    };
    use std::{
        collections::HashMap,
        sync::Mutex,
        time::{Duration, Instant},
    };
    let (device, queue) = gpu();
    let mut res = Resources::new(&device);
    res.keep_globe = true;
    let ctx = egui::Context::default();
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280., 832.));
    let painter = ctx
        .layer_painter(egui::LayerId::background())
        .with_clip_rect(rect);
    let mut state = State::default();
    let builds = Arc::new(Mutex::new(HashMap::<Key, usize>::new()));
    let overview = Raster::new(
        HeightGrid {
            width: 129,
            height: 65,
            metres: vec![1000.; 129 * 65],
        },
        [-180., -90., 180., 90.],
    )
    .unwrap();
    let make_tile = |key: Key, side: usize| {
        let mut raster = Raster::new(
            HeightGrid {
                width: side,
                height: side,
                metres: vec![1500.; side * side],
            },
            key.region().bounds(),
        )
        .unwrap();
        Arc::get_mut(&mut raster).unwrap().core = Some(key.bounds());
        raster
    };
    let prepare = |res: &mut Resources, cb: &Callback| {
        let mut encoder = device.create_command_encoder(&Default::default());
        res.prepare(&device, &queue, &mut encoder, cb, 1.);
        queue.submit([encoder.finish()]);
        device.poll(wgpu::Maintain::Wait);
    };
    let mut retained_bytes = 0;
    for round in 0..2 {
        for (lat, lon, zoom) in [(39., -98., 3.), (40., -110., 32.), (38., -79., 38.)] {
            let start = Instant::now();
            let mut view = GlobeViewState::from_focus(GeoPoint { lat, lon });
            view.zoom = zoom;
            let layout = globe_layout(rect, &view);
            let wanted = tiles::selection(rect, &layout, &view);
            let frame = loop {
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
                        Ok(make_tile(key, 65))
                    },
                );
                if frame.missing == 0 {
                    break frame;
                }
                assert_eq!(
                    round, 0,
                    "warm camera path must return all tiles immediately"
                );
                assert!(start.elapsed() < Duration::from_secs(10));
                std::thread::sleep(Duration::from_millis(1));
            };
            let ids: Vec<_> = frame.rasters.iter().map(|r| r.id).collect();
            let mut data = vec![overview.clone()];
            data.extend(frame.rasters);
            let cb = Callback::globe(&painter, rect, &layout, &view, data, false, false, 1.);
            prepare(&mut res, &cb);
            assert!(
                ids.iter().all(|id| res.slots[0].active.contains(id)),
                "all previously seen tiles draw on first frame"
            );
            for key in &wanted {
                assert_eq!(res.slots[0].coverage_bytes[key.1 * tiles::COLS + key.0], 1);
            }
            if round > 0 {
                assert_eq!(
                    res.uploaded_bytes, retained_bytes,
                    "camera revisit uploads zero bytes"
                );
            }
            eprintln!(
                "Atlas camera round={round} ({lat},{lon},{zoom}): {} active tiles, {:.2} ms",
                ids.len(),
                start.elapsed().as_secs_f64() * 1000.
            );
        }
        retained_bytes = res.uploaded_bytes;
    }
    assert!(builds.lock().unwrap().values().all(|&count| count == 1));
    // A slow new upload must not block a ready neighbor, or cut a hole in the overview.
    let ready = make_tile(Key(12, 24), 65);
    let partial = make_tile(Key(13, 24), 2048);
    let view = GlobeViewState::from_focus(GeoPoint {
        lat: 32.,
        lon: -114.,
    });
    let cb = Callback::globe(
        &painter,
        rect,
        &globe_layout(rect, &view),
        &view,
        vec![overview.clone(), ready.clone(), partial.clone()],
        false,
        false,
        1.,
    );
    prepare(&mut res, &cb);
    assert!(res.slots[0].active.contains(&ready.id));
    assert!(!res.slots[0].active.contains(&partial.id));
    assert_eq!(res.slots[0].coverage_bytes[24 * tiles::COLS + 12], 1);
    assert_eq!(res.slots[0].coverage_bytes[24 * tiles::COLS + 13], 0);
    let cb = Callback::globe(
        &painter,
        rect,
        &globe_layout(rect, &view),
        &view,
        vec![overview, ready],
        false,
        false,
        1.,
    );
    let before = res.uploaded_bytes;
    prepare(&mut res, &cb);
    assert_eq!(res.uploaded_bytes, before);
    assert!(!res.slots[0].sources.contains_key(&partial.id));
}
