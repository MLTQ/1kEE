use super::*;
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
#[ignore = "requires a working GPU; renders into a private offscreen texture"]
fn roads_and_contours_render_together_and_deferred_uploads_complete() {
    let instance = wgpu::Instance::default();
    let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
        .expect("GPU adapter");
    let (device, queue) =
        block_on(adapter.request_device(&wgpu::DeviceDescriptor::default(), None)).unwrap();
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut renderer = egui_wgpu::Renderer::new(&device, format, None, 1, false);
    renderer
        .callback_resources
        .insert(LocalContourPassResources::new(&device, format));
    let ctx = egui::Context::default();
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(64.0, 64.0));
    let params = LocalProjectionParams {
        focus_lat: 0.0,
        focus_lon: 0.0,
        x_factor: 1.0,
        y_factor: 1.0,
        z_factor: 0.0,
        yaw_cos: 1.0,
        yaw_sin: 0.0,
        pitch_cos: 1.0,
        pitch_sin: 0.0,
        focus_center_x: 0.0,
        focus_center_y: 0.0,
        horizontal_scale: 1.0,
        ground_pitch_scale: 1.0,
        ground_depth_scale: 0.0,
        elevation_pitch_scale: 0.0,
        elevation_depth_scale: 0.0,
    };
    let road_batches: Vec<_> = (0..4)
        .map(|chunk| LocalTileBatch {
            id: LocalBatchId::Road { major: true, chunk },
            version: 17,
            instances: Arc::new(vec![LocalSegmentInstance::line(
                [8.0 + chunk as f32 * 12.0, -32.0, 0.0],
                [20.0 + chunk as f32 * 12.0, -32.0, 0.0],
                egui::Color32::WHITE,
                true,
            )]),
        })
        .collect();
    let contour = LocalTileBatch {
        id: LocalBatchId::Contour(LocalTileId {
            zoom_bucket: 0,
            lat_bucket: 0,
            lon_bucket: 0,
        }),
        version: 17,
        instances: Arc::new(vec![LocalSegmentInstance::line(
            [8.0, -16.0, 0.0],
            [56.0, -16.0, 0.0],
            egui::Color32::WHITE,
            false,
        )]),
    };
    let screen = egui_wgpu::ScreenDescriptor {
        size_in_pixels: [64, 64],
        pixels_per_point: 1.0,
    };
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("road regression target"),
        size: wgpu::Extent3d {
            width: 64,
            height: 64,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let target_view = target.create_view(&Default::default());
    for frame in 0..2 {
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(rect),
                ..Default::default()
            },
            |ctx| {
                let painter = ctx.layer_painter(egui::LayerId::new(
                    egui::Order::Foreground,
                    egui::Id::new("test"),
                ));
                painter.add(
                    LocalContourCallback::new(
                        LocalContourPass::Surface,
                        vec![contour.clone()],
                        &params,
                        1.0,
                        2.0,
                        2.0,
                        ctx.clone(),
                    )
                    .into_paint_callback(rect),
                );
                painter.add(
                    LocalContourCallback::new(
                        LocalContourPass::Roads,
                        road_batches.clone(),
                        &params,
                        1.0,
                        4.0,
                        4.0,
                        ctx.clone(),
                    )
                    .into_paint_callback(rect),
                );
            },
        );
        let jobs = ctx.tessellate(output.shapes, output.pixels_per_point);
        let mut encoder = device.create_command_encoder(&Default::default());
        let commands = renderer.update_buffers(&device, &queue, &mut encoder, &jobs, &screen);
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("road regression"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            renderer.render(&mut pass.forget_lifetime(), &jobs, &screen);
        }
        queue.submit(
            commands
                .into_iter()
                .chain(std::iter::once(encoder.finish())),
        );
        device.poll(wgpu::Maintain::Wait);
        let res = renderer
            .callback_resources
            .get::<LocalContourPassResources>()
            .unwrap();
        assert_eq!(res.tiles.len(), if frame == 0 { 4 } else { 5 });
    }
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 64 * 64 * 4,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        target.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(256),
                rows_per_image: Some(64),
            },
        },
        wgpu::Extent3d {
            width: 64,
            height: 64,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);
    let (tx, rx) = std::sync::mpsc::channel();
    readback
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            tx.send(result).unwrap();
        });
    device.poll(wgpu::Maintain::Wait);
    rx.recv().unwrap().unwrap();
    let pixels = readback.slice(..).get_mapped_range();
    // The four road pieces must be continuous, including all three joins.
    for x in 9..55 {
        assert!(pixels[(32 * 64 + x) * 4] > 200, "road gap at x={x}");
        assert!(pixels[(16 * 64 + x) * 4] > 200, "missing contour at x={x}");
    }
    // Separate slots preserve the contour's thinner stroke alongside roads.
    assert!(pixels[(14 * 64 + 30) * 4] < 100);
    assert!(pixels[(30 * 64 + 30) * 4] > 200);
}
