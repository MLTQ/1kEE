//! Optional GPU rendering of the actual event overlay, using test-only stories.
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
            Poll::Pending => std::thread::park_timeout(std::time::Duration::from_millis(10)),
        }
    }
}

#[test]
#[ignore = "requires GPU; optional ONEKEE_EVENT_OVERLAY_PREVIEW directory"]
fn five_line_chyron_gpu_preview() {
    let adapter = block_on(wgpu::Instance::default().request_adapter(&Default::default()))
        .expect("GPU adapter");
    let (device, queue) = block_on(adapter.request_device(&Default::default(), None)).unwrap();
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("event overlay visual QA"),
        size: wgpu::Extent3d {
            width: 1280,
            height: 720,
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
        size_in_pixels: [1280, 720],
        pixels_per_point: 1.0,
    };
    let ctx = egui::Context::default();
    crate::theme::install(&ctx);
    let mut model = model();
    model.stop_event_follow();
    for (index, event) in model.events.iter_mut().enumerate() {
        event.title = format!("Preview story {index}");
        event.summary = format!(
            "This sample report demonstrates a complete scrolling summary, with its final details retained while the camera moves to another event. Additional context stays readable rather than being shortened with an ellipsis. Report {index}."
        );
        event.severity = EventSeverity::Elevated;
        event.factal_brief.as_mut().unwrap().severity_value = Some(2);
    }
    model.events[7].title = "Russia-Ukraine Conflict".into();
    model.events[7].summary = "Mayor reports power and water outages in Kyiv, Ukraine, after Russian strikes on multiple critical infrastructure facilities".into();
    model.events[7].severity = EventSeverity::Critical;
    model.events[7]
        .factal_brief
        .as_mut()
        .unwrap()
        .severity_value = Some(5);
    model.toggle_event_follow();
    model.tick_event_follow(0.0);
    model.obs_view = true;
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 720.0));
    let markers: Vec<_> = model
        .events
        .iter()
        .enumerate()
        .map(|(index, event)| {
            let base = egui::pos2(
                260.0 + index as f32 * 28.0,
                300.0 + (index % 3) as f32 * 45.0,
            );
            marker_style::MapMarker::new(&event.id, base, base, 1.0)
        })
        .collect();
    let mut renderer = egui_wgpu::Renderer::new(&device, format, None, 1, false);
    let dir = std::env::var_os("ONEKEE_EVENT_OVERLAY_PREVIEW")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("target/event-chyron-qa"));
    std::fs::create_dir_all(&dir).unwrap();
    for (index, time) in [15.0, 15.05, 15.1, 21.1].into_iter().enumerate() {
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(rect),
                time: Some(time),
                ..Default::default()
            },
            |ctx| {
                super::super::super::render_factal_brief(ctx, &mut model);
                let painter = ctx.layer_painter(egui::LayerId::background());
                painter.rect_filled(rect, 0.0, crate::theme::canvas_background());
                for x in (0..1280).step_by(80) {
                    painter.line_segment(
                        [egui::pos2(x as f32, 0.0), egui::pos2(x as f32, 720.0)],
                        egui::Stroke::new(0.5, crate::theme::grid_color().gamma_multiply(0.3)),
                    );
                }
                for y in (0..720).step_by(80) {
                    painter.line_segment(
                        [egui::pos2(0.0, y as f32), egui::pos2(1280.0, y as f32)],
                        egui::Stroke::new(0.5, crate::theme::grid_color().gamma_multiply(0.3)),
                    );
                }
                for marker in &markers {
                    painter.circle_filled(marker.base, 3.5, EventSeverity::Critical.color());
                }
                draw_event_follow_overlay(&painter, rect, &model, &markers);
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
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 1280 * 720 * 4,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        if index >= 2 {
            encoder.copy_texture_to_buffer(
                target.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &readback,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(5120),
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
        if index >= 2 {
            let (tx, rx) = std::sync::mpsc::channel();
            readback
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |result| tx.send(result).unwrap());
            device.poll(wgpu::Maintain::Wait);
            rx.recv().unwrap().unwrap();
            let name = if index == 2 {
                "chyron-start.png"
            } else {
                "chyron-scrolling.png"
            };
            let pixels = readback.slice(..).get_mapped_range();
            assert!(
                pixels.chunks_exact(4).any(|pixel| pixel[0] > 100),
                "text rendered on the GPU"
            );
            image::save_buffer(dir.join(name), &pixels, 1280, 720, image::ColorType::Rgba8)
                .unwrap();
        }
    }
}
