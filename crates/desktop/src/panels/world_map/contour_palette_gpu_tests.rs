//! Compare native line output with egui's actual color and opacity handling.
use super::*;
use std::task::{Context, Poll, Wake, Waker};

fn block_on<T>(future: impl std::future::Future<Output = T>) -> T {
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
#[ignore = "requires GPU; compares native contour pixels with egui on both framebuffer formats"]
fn contour_palette_and_fades_match_egui_across_tile_handoff() {
    let adapter = block_on(wgpu::Instance::default().request_adapter(&Default::default()))
        .expect("GPU adapter");
    let (device, queue) = block_on(adapter.request_device(&Default::default(), None)).unwrap();
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(64.0, 64.0));
    let screen = egui_wgpu::ScreenDescriptor {
        size_in_pixels: [64, 64],
        pixels_per_point: 1.0,
    };
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
    let cases = [
        ("Topo orange", egui::Color32::from_rgb(245, 125, 78), 1.0),
        (
            "Topo minor",
            egui::Color32::from_rgb(96, 164, 181).gamma_multiply(0.78),
            1.0,
        ),
        ("Topo fade", egui::Color32::from_rgb(245, 125, 78), 0.35),
        ("Phosphor amber", egui::Color32::from_rgb(210, 162, 40), 1.0),
        ("Sodium amber", egui::Color32::from_rgb(255, 170, 72), 1.0),
    ];
    let mut failures = Vec::new();
    for format in [
        wgpu::TextureFormat::Rgba8Unorm,
        wgpu::TextureFormat::Rgba8UnormSrgb,
    ] {
        // The backdrop shares the output transfer helper but shades in linear
        // light. Validate its real shader specialization on both targets too.
        let _backdrop = super::super::globe_pass::GlobePassResources::new(&device, format);
        let ctx = egui::Context::default();
        let mut renderer = egui_wgpu::Renderer::new(&device, format, None, 1, false);
        renderer
            .callback_resources
            .insert(LocalContourPassResources::new(&device, format));
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("contour palette parity"),
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
        let view = target.create_view(&Default::default());
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 64 * 64 * 4,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        for (index, (name, color, alpha)) in cases.iter().enumerate() {
            for live_palette in [false, true] {
                let batch = LocalTileBatch {
                    id: LocalBatchId::Contour(LocalTileId {
                        zoom_bucket: 0,
                        lat_bucket: 0,
                        lon_bucket: 0,
                    }),
                    version: index as u64 + 1,
                    occlusion: Default::default(),
                    instances: Arc::new(vec![LocalSegmentInstance::line(
                        [40.0, -32.5, 0.0],
                        [56.0, -32.5, 0.0],
                        *color,
                        true,
                    )]),
                };
                let output = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(rect),
                        ..Default::default()
                    },
                    |ctx| {
                        let painter = ctx.layer_painter(egui::LayerId::background());
                        painter.rect_filled(rect, 0, egui::Color32::from_rgb(5, 15, 22));
                        painter.line_segment(
                            [egui::pos2(8.0, 32.5), egui::pos2(24.0, 32.5)],
                            egui::Stroke::new(4.0, color.gamma_multiply(*alpha)),
                        );
                        painter.add(residency_callback(rect, ctx));
                        let mut callback = LocalContourCallback::new(
                            LocalContourPass::Surface,
                            vec![batch.clone()],
                            &params,
                            *alpha,
                            4.0,
                            4.0,
                            ctx.clone(),
                        );
                        if live_palette {
                            callback = callback.with_palette(*color, *color);
                        }
                        painter.add(callback.into_paint_callback(rect));
                    },
                );
                for (id, delta) in &output.textures_delta.set {
                    renderer.update_texture(&device, &queue, *id, delta);
                }
                let jobs = ctx.tessellate(output.shapes, output.pixels_per_point);
                let mut encoder = device.create_command_encoder(&Default::default());
                let commands =
                    renderer.update_buffers(&device, &queue, &mut encoder, &jobs, &screen);
                {
                    let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: None,
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &view,
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
                    target.size(),
                );
                queue.submit(
                    commands
                        .into_iter()
                        .chain(std::iter::once(encoder.finish())),
                );
                let (tx, rx) = std::sync::mpsc::channel();
                readback
                    .slice(..)
                    .map_async(wgpu::MapMode::Read, move |result| tx.send(result).unwrap());
                device.poll(wgpu::Maintain::Wait);
                rx.recv().unwrap().unwrap();
                let pixels = readback.slice(..).get_mapped_range();
                let pixel = |x: usize| -> [u8; 4] {
                    pixels[(32 * 64 + x) * 4..(32 * 64 + x + 1) * 4]
                        .try_into()
                        .unwrap()
                };
                let (cpu, gpu) = (pixel(16), pixel(48));
                println!(
                    "{format:?} {name}, live palette={live_palette}: egui={cpu:?}, GPU={gpu:?}"
                );
                if cpu.iter().zip(gpu).any(|(a, b)| a.abs_diff(b) > 2) {
                    failures.push(format!(
                        "{format:?} {name} palette={live_palette}: {cpu:?} != {gpu:?}"
                    ));
                }
                drop(pixels);
                readback.unmap();
            }
        }
    }
    assert!(
        failures.is_empty(),
        "CPU/GPU color mismatch:\n{}",
        failures.join("\n")
    );
}
