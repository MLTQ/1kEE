use super::*;
use crate::panels::world_map::globe_scene::GlobeLayout;
use std::future::Future;
use std::task::{Context, Poll, Wake, Waker};

fn block_on<T>(future: impl Future<Output = T>) -> T {
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
#[ignore = "requires a working GPU; exercises real egui callback preparation"]
fn globe_gpu_layers_release_buffers_during_repeated_mode_changes() {
    let instance = wgpu::Instance::default();
    let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
        .expect("GPU adapter");
    let (device, queue) =
        block_on(adapter.request_device(&wgpu::DeviceDescriptor::default(), None)).unwrap();
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut renderer = egui_wgpu::Renderer::new(&device, format, None, 1, false);
    renderer
        .callback_resources
        .insert(ContourPassResources::new(&device, format));
    let ctx = egui::Context::default();
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(64.0, 64.0));
    let layout = GlobeLayout {
        center: rect.center(),
        radius: 30.0,
        focal_length: 2.0,
        camera_distance: 3.0,
    };
    let view = GlobeViewState::from_focus(GeoPoint { lat: 0.0, lon: 0.0 });
    let instances = Arc::new(vec![
        SegmentInstance::line(
            GeoPoint { lat: 0.0, lon: 0.0 },
            GeoPoint { lat: 0.1, lon: 0.1 },
            egui::Color32::WHITE,
        );
        8192
    ]);
    let screen = egui_wgpu::ScreenDescriptor {
        size_in_pixels: [64, 64],
        pixels_per_point: 1.0,
    };
    for frame in 0..60 {
        let count = 2 - frame % 3;
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(rect),
                ..Default::default()
            },
            |ctx| {
                let painter = ctx.layer_painter(egui::LayerId::new(
                    egui::Order::Foreground,
                    egui::Id::new("globe-lifetime-test"),
                ));
                painter.add(residency_callback(rect));
                for layer in [ContourLayer::Coastlines, ContourLayer::SrtmGlobe]
                    .into_iter()
                    .take(count)
                {
                    painter.add(
                        ContourCallback::new(
                            layer,
                            frame as u64,
                            instances.clone(),
                            &layout,
                            &view,
                            0.02,
                            1.0,
                            1.0,
                            1.0,
                        )
                        .into_paint_callback(rect),
                    );
                }
            },
        );
        let jobs = ctx.tessellate(output.shapes, output.pixels_per_point);
        let mut encoder = device.create_command_encoder(&Default::default());
        let commands = renderer.update_buffers(&device, &queue, &mut encoder, &jobs, &screen);
        queue.submit(
            commands
                .into_iter()
                .chain(std::iter::once(encoder.finish())),
        );
        device.poll(wgpu::Maintain::Wait);
        let res = renderer
            .callback_resources
            .get::<ContourPassResources>()
            .unwrap();
        assert_eq!(res.layers.len(), count, "frame {frame}");
        let bytes: u64 = res
            .layers
            .values()
            .flat_map(|layer| &layer.chunks)
            .map(|chunk| chunk.buffer.size())
            .sum();
        assert_eq!(
            bytes,
            (count * instances.len() * std::mem::size_of::<SegmentInstance>()) as u64
        );
    }
}

#[test]
#[ignore = "requires GPU; large replacements must upload incrementally"]
fn large_layer_upload_retains_previous_generation_until_complete() {
    use egui_wgpu::CallbackTrait;
    let adapter = block_on(wgpu::Instance::default().request_adapter(&Default::default())).unwrap();
    let (device, queue) = block_on(adapter.request_device(&Default::default(), None)).unwrap();
    let mut resources = egui_wgpu::CallbackResources::default();
    resources.insert(ContourPassResources::new(
        &device,
        wgpu::TextureFormat::Rgba8Unorm,
    ));
    let layout = GlobeLayout {
        center: egui::Pos2::ZERO,
        radius: 30.,
        focal_length: 2.,
        camera_distance: 3.,
    };
    let view = GlobeViewState::from_focus(GeoPoint { lat: 0., lon: 0. });
    let segment = SegmentInstance::line(
        GeoPoint { lat: 0., lon: 0. },
        GeoPoint { lat: 1., lon: 1. },
        egui::Color32::WHITE,
    );
    let small = Arc::new(vec![segment]);
    let large = Arc::new(vec![segment; 700_000]);
    let screen = egui_wgpu::ScreenDescriptor {
        size_in_pixels: [64, 64],
        pixels_per_point: 1.,
    };
    for frame in 0..100 {
        let mut encoder = device.create_command_encoder(&Default::default());
        Cleanup.prepare(&device, &queue, &screen, &mut encoder, &mut resources);
        let callback = ContourCallback::new(
            ContourLayer::Coastlines,
            if frame == 0 { 1 } else { 2 },
            if frame == 0 {
                small.clone()
            } else {
                large.clone()
            },
            &layout,
            &view,
            0.02,
            1.,
            1.,
            1.,
        );
        callback.prepare(&device, &queue, &screen, &mut encoder, &mut resources);
        Cleanup.finish_prepare(&device, &queue, &mut encoder, &mut resources);
        queue.submit([encoder.finish()]);
        device.poll(wgpu::Maintain::Wait);
        let layer =
            &resources.get::<ContourPassResources>().unwrap().layers[&ContourLayer::Coastlines];
        if layer.version == 2 {
            assert!(frame > 1, "one large layer cannot upload in one frame");
            assert_eq!(
                layer.chunks.iter().map(|c| c.count as usize).sum::<usize>(),
                large.len()
            );
            assert!(layer.chunks.iter().all(|c| c.buffer.size() <= 512 * 1024));
            return;
        }
        assert_eq!(layer.version, 1);
        assert_eq!(
            layer.chunks.iter().map(|c| c.count).sum::<u32>(),
            1,
            "retain complete previous layer while uploading"
        );
    }
    panic!("staged layer never completed");
}
