//! Opt-in rendering of the actual radio window with inert test-only fixtures.
use super::*;
use eframe::{egui_wgpu, wgpu};

fn block_on<T>(future: impl std::future::Future<Output = T>) -> T {
    use std::{
        sync::Arc,
        task::{Context, Poll, Wake, Waker},
    };
    struct Unpark(std::thread::Thread);
    impl Wake for Unpark {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = Waker::from(Arc::new(Unpark(std::thread::current())));
    let mut cx = Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(value) => return value,
            Poll::Pending => std::thread::park_timeout(Duration::from_millis(10)),
        }
    }
}

#[test]
#[ignore = "requires GPU; writes native radio-window preview PNGs"]
fn radio_gpu_preview() {
    let adapter = block_on(wgpu::Instance::default().request_adapter(&Default::default()))
        .expect("GPU adapter");
    let (device, queue) = block_on(adapter.request_device(&Default::default(), None)).unwrap();
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let (width, height) = (1024, 768);
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("radio window QA"),
        size: wgpu::Extent3d {
            width,
            height,
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
    let screen = egui_wgpu::ScreenDescriptor {
        size_in_pixels: [width, height],
        pixels_per_point: 1.0,
    };
    for (name, empty, add) in [
        ("radio-results", false, false),
        ("radio-empty", true, false),
        ("radio-add-stream", false, true),
    ] {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut radio = primed();
        radio.open_for(&ctx, &event("preview", 50.67, -120.34));
        if empty {
            radio.kind = Some(RadioKind::Ems);
        }
        radio.add_open = add;
        let mut renderer = egui_wgpu::Renderer::new(&device, format, None, 1, false);
        for frame in 0..4 {
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(width as f32, height as f32),
                    )),
                    time: Some(frame as f64 * 0.1),
                    ..Default::default()
                },
                |ctx| {
                    let painter = ctx.layer_painter(egui::LayerId::background());
                    painter.rect_filled(ctx.screen_rect(), 0.0, theme::canvas_background());
                    radio.render(ctx, None);
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
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: (width * height * 4) as u64,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            if frame == 3 {
                encoder.copy_texture_to_buffer(
                    target.as_image_copy(),
                    wgpu::TexelCopyBufferInfo {
                        buffer: &buffer,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(width * 4),
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
            if frame == 3 {
                let (sender, receiver) = std::sync::mpsc::channel();
                buffer
                    .slice(..)
                    .map_async(wgpu::MapMode::Read, move |r| sender.send(r).unwrap());
                device.poll(wgpu::Maintain::Wait);
                receiver.recv().unwrap().unwrap();
                let dir = std::env::var("ONEKEE_RADIO_PREVIEW").unwrap_or_else(|_| {
                    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("../../target/radio-qa")
                        .to_string_lossy()
                        .into_owned()
                });
                std::fs::create_dir_all(&dir).unwrap();
                image::save_buffer(
                    std::path::Path::new(&dir).join(format!("{name}.png")),
                    &buffer.slice(..).get_mapped_range(),
                    width,
                    height,
                    image::ColorType::Rgba8,
                )
                .unwrap();
            }
        }
    }
}
