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
#[test]
#[ignore = "requires GPU; verifies zero re-upload after switching Atlas modes and explicit invalidation"]
fn atlas_gpu_revisit_reuses_uploaded_sources_and_render_targets() {
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
    let (device, queue) = block_on(adapter.request_device(&Default::default(), None)).unwrap();
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
