use super::*;

#[test]
fn picking_tracks_size_and_lit_spire_without_intercepting_transparent_tips() {
    let base = egui::pos2(100.0, 200.0);
    for scale in [0.25, 1.0, 2.0] {
        let marker = MapMarker::new("camera", base, local_tip(base, 76.0, scale), scale);
        assert!(marker.hit_distance(base).is_some());
        assert!(marker.hit_distance(base.lerp(marker.tip, 0.4)).is_some());
        assert!(marker.hit_distance(marker.tip).is_none());
        assert!(marker.hit_distance(base + egui::vec2(24.0, 0.0)).is_none());
    }
    let small = MapMarker::new("small", base, base, 0.25);
    let large = MapMarker::new("large", base, base, 2.0);
    assert!(small.hit_distance(base + egui::vec2(12.0, 0.0)).is_none());
    assert!(large.hit_distance(base + egui::vec2(12.0, 0.0)).is_some());
}

#[test]
fn crowded_picking_chooses_the_closest_ground_anchor_before_a_crossing_spire() {
    let markers = vec![
        MapMarker::new(
            "far",
            egui::pos2(100.0, 100.0),
            egui::pos2(100.0, 24.0),
            1.0,
        ),
        MapMarker::new(
            "near",
            egui::pos2(105.0, 100.0),
            egui::pos2(105.0, 24.0),
            1.0,
        ),
        MapMarker::new(
            "crossing",
            egui::pos2(105.0, 140.0),
            egui::pos2(105.0, 64.0),
            1.0,
        ),
    ];
    assert_eq!(pick(&markers, egui::pos2(106.0, 100.0)).unwrap().id, "near");
    assert!(pick(&markers, egui::pos2(200.0, 100.0)).is_none());
}

fn block_on<T>(future: impl std::future::Future<Output = T>) -> T {
    use std::sync::Arc;
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
#[ignore = "requires a GPU; exports only when ONEKEE_MARKER_PREVIEW is set"]
fn marker_spires_render_at_small_normal_and_large_sizes() {
    use eframe::{egui_wgpu, wgpu};
    let adapter = block_on(wgpu::Instance::default().request_adapter(&Default::default()))
        .expect("GPU adapter");
    let (device, queue) = block_on(adapter.request_device(&Default::default(), None)).unwrap();
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut renderer = egui_wgpu::Renderer::new(&device, format, None, 1, false);
    let ctx = egui::Context::default();
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1024.0, 384.0),
            )),
            ..Default::default()
        },
        |ctx| {
            let painter = ctx.layer_painter(egui::LayerId::background());
            for (index, scale) in [0.25, 1.0, 2.0].into_iter().enumerate() {
                let x = 170.0 + index as f32 * 342.0;
                painter.text(
                    egui::pos2(x, 25.0),
                    egui::Align2::CENTER_TOP,
                    format!("{}%", (scale * 100.0) as i32),
                    egui::FontId::proportional(18.0),
                    egui::Color32::WHITE,
                );
                let base = egui::pos2(x - 65.0, 305.0);
                draw_camera_spire(&painter, base, local_tip(base, 76.0, scale), false, scale);
                // Globe surface-normal projection can slant a spire away from the viewer.
                let globe_base = egui::pos2(x + 35.0, 305.0);
                let tip = globe_base + egui::vec2(30.0, -70.0) * scale;
                draw_camera_spire(&painter, globe_base, tip, true, scale);
                painter.text(
                    base + egui::vec2(0.0, 35.0),
                    egui::Align2::CENTER_TOP,
                    "Local",
                    egui::FontId::proportional(14.0),
                    egui::Color32::LIGHT_GRAY,
                );
                painter.text(
                    globe_base + egui::vec2(0.0, 35.0),
                    egui::Align2::CENTER_TOP,
                    "Globe / selected",
                    egui::FontId::proportional(14.0),
                    egui::Color32::LIGHT_GRAY,
                );
            }
        },
    );
    for (id, delta) in &output.textures_delta.set {
        renderer.update_texture(&device, &queue, *id, delta);
    }
    let jobs = ctx.tessellate(output.shapes, output.pixels_per_point);
    let screen = egui_wgpu::ScreenDescriptor {
        size_in_pixels: [1024, 384],
        pixels_per_point: 1.0,
    };
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("marker size preview"),
        size: wgpu::Extent3d {
            width: 1024,
            height: 384,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let mut encoder = device.create_command_encoder(&Default::default());
    let commands = renderer.update_buffers(&device, &queue, &mut encoder, &jobs, &screen);
    {
        let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("marker preview"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.024,
                        g: 0.055,
                        b: 0.070,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        renderer.render(&mut pass.forget_lifetime(), &jobs, &screen);
    }
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 1024 * 384 * 4,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        target.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4096),
                rows_per_image: None,
            },
        },
        target.size(),
    );
    queue.submit(
        commands
            .into_iter()
            .chain(std::iter::once(encoder.finish())),
    );
    let slice = readback.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        tx.send(r).unwrap();
    });
    device.poll(wgpu::Maintain::Wait);
    rx.recv().unwrap().unwrap();
    let pixels = slice.get_mapped_range();
    // The core should extend increasingly far from the same baseline as size grows.
    for (index, scale) in [0.25, 1.0, 2.0].into_iter().enumerate() {
        let x = 105 + index * 342;
        let y = (305.0 - 76.0 * scale * 0.35) as usize;
        let offset = (y * 1024 + x) * 4;
        assert!(pixels[offset + 1] > 22, "spire missing at {scale}x");
    }
    if let Ok(path) = std::env::var("ONEKEE_MARKER_PREVIEW") {
        image::save_buffer(path, &pixels, 1024, 384, image::ColorType::Rgba8).unwrap();
    }
}
