//! Opt-in real-data GPU preview and capacity/product switching regression.
use super::super::{contour_pass::ContourPassResources, pipeline_capacity};
use super::*;
use eframe::{egui_wgpu, wgpu};

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
#[ignore = "requires GPU and ONEKEE_PIPELINE_ARCHIVE; optional ONEKEE_CAPACITY_PREVIEW PNG"]
fn capacity_globe_preview_and_mode_round_trip() {
    let path = std::env::var("ONEKEE_PIPELINE_ARCHIVE").unwrap();
    let reader = tile_archive::Reader::open(Path::new(&path)).unwrap();
    let features = tile_archive::pipelines::load(&reader, None).unwrap();
    assert!(features.iter().any(|f| f.info.capacity.is_some()));
    let product_colors = [
        egui::Color32::from_rgb(100, 200, 150),
        egui::Color32::from_rgb(200, 140, 100),
        egui::Color32::from_rgb(200, 200, 100),
    ];
    let (product, product_owners) = prepare(&features, Filter::default(), false, product_colors);
    let (capacity, capacity_owners) = prepare(&features, Filter::default(), true, product_colors);
    assert_eq!(product_owners, capacity_owners);
    assert_eq!(product.len(), capacity.len());
    assert!(
        product
            .iter()
            .zip(&capacity)
            .all(|(a, b)| a.endpoints() == b.endpoints())
    );
    let batches = [Arc::new(product), Arc::new(capacity)];
    let adapter = block_on(wgpu::Instance::default().request_adapter(&Default::default()))
        .expect("GPU adapter");
    let (device, queue) = block_on(adapter.request_device(&Default::default(), None)).unwrap();
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut renderer = egui_wgpu::Renderer::new(&device, format, None, 1, false);
    renderer
        .callback_resources
        .insert(ContourPassResources::new(&device, format));
    let ctx = egui::Context::default();
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1024.0, 640.0));
    let map = egui::Rect::from_min_max(egui::pos2(380.0, 0.0), rect.max);
    let layout = GlobeLayout {
        center: map.center(),
        radius: 400.0,
        focal_length: 2.0,
        camera_distance: 3.0,
    };
    let view = GlobeViewState::from_focus(GeoPoint {
        lat: 30.0,
        lon: 25.0,
    });
    let screen = egui_wgpu::ScreenDescriptor {
        size_in_pixels: [1024, 640],
        pixels_per_point: 1.0,
    };
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("pipeline capacity QA"),
        size: wgpu::Extent3d {
            width: 1024,
            height: 640,
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
    let mut pictures = Vec::new();
    for (version, mode) in [0usize, 1, 0].into_iter().enumerate() {
        // Complete the normal staged GPU upload, including the largest real overview.
        let frames =
            (batches[mode].len() * std::mem::size_of::<SegmentInstance>()).div_ceil(512 * 1024) + 8;
        for frame in 0..frames {
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(rect),
                    ..Default::default()
                },
                |ctx| {
                    egui::SidePanel::left("legend")
                        .exact_width(355.0)
                        .resizable(false)
                        .show(ctx, |ui| {
                            ui.add_space(16.0);
                            pipeline_capacity::legend(ui);
                            ui.add_space(14.0);
                            ui.small("Global Energy Monitor · CC BY 4.0");
                            ui.small("Gas: November 2025 · Oil: June 2026");
                            ui.small("Public map snapshots. Routes may be approximate.");
                        });
                    let painter = ctx
                        .layer_painter(egui::LayerId::background())
                        .with_clip_rect(map);
                    painter.add(super::super::contour_pass::residency_callback(rect));
                    painter.circle_stroke(
                        layout.center,
                        layout.radius * layout.focal_length
                            / (layout.camera_distance * layout.camera_distance - 1.0).sqrt(),
                        egui::Stroke::new(1.0, egui::Color32::from_gray(55)),
                    );
                    painter.add(
                        ContourCallback::new(
                            ContourLayer::Pipelines,
                            version as u64 + 1,
                            batches[mode].clone(),
                            &layout,
                            &view,
                            0.0,
                            1.0,
                            1.5,
                            1.0,
                        )
                        .into_paint_callback(map),
                    );
                    pipeline_capacity::map_label(&painter);
                },
            );
            for (id, delta) in &output.textures_delta.set {
                renderer.update_texture(&device, &queue, *id, delta);
            }
            let jobs = ctx.tessellate(output.shapes, output.pixels_per_point);
            let mut encoder = device.create_command_encoder(&Default::default());
            let commands = renderer.update_buffers(&device, &queue, &mut encoder, &jobs, &screen);
            {
                let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: None,
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &target_view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color {
                                r: 0.025,
                                g: 0.035,
                                b: 0.045,
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
                size: 1024 * 640 * 4,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            if frame + 1 == frames {
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
            }
            queue.submit(
                commands
                    .into_iter()
                    .chain(std::iter::once(encoder.finish())),
            );
            device.poll(wgpu::Maintain::Wait);
            if frame + 1 == frames {
                let (tx, rx) = std::sync::mpsc::channel();
                readback
                    .slice(..)
                    .map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
                device.poll(wgpu::Maintain::Wait);
                rx.recv().unwrap().unwrap();
                pictures.push(readback.slice(..).get_mapped_range().to_vec());
            }
        }
    }
    let map_pixels = |pixels: &[u8]| {
        pixels
            .chunks_exact(4096)
            .flat_map(|r| r[420 * 4..].iter().copied())
            .collect::<Vec<_>>()
    };
    if let Ok(path) = std::env::var("ONEKEE_CAPACITY_PREVIEW") {
        image::save_buffer(&path, &pictures[1], 1024, 640, image::ColorType::Rgba8).unwrap();
        for (i, picture) in pictures.iter().enumerate() {
            image::save_buffer(
                format!("{path}.{i}.png"),
                picture,
                1024,
                640,
                image::ColorType::Rgba8,
            )
            .unwrap();
        }
    }
    assert!(
        map_pixels(&pictures[0]) != map_pixels(&pictures[1]),
        "capacity mode must change the rendered data"
    );
    let changed = map_pixels(&pictures[0])
        .iter()
        .zip(map_pixels(&pictures[2]))
        .filter(|(a, b)| **a != *b)
        .count();
    assert!(
        changed == 0,
        "returning to product mode restores the image: {changed} changed channels"
    );
}
