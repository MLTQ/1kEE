//! Optional GPU verification of actual admin controls and rendering.
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
    let mut context = Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(v) => return v,
            Poll::Pending => std::thread::park_timeout(std::time::Duration::from_millis(10)),
        }
    }
}
#[test]
#[ignore = "requires GPU; optional ONEKEE_ADMIN_PREVIEW PNG"]
fn admin_gpu_preview_preserves_holes_and_toggle_round_trip() {
    let scratch = tests::Scratch::new();
    let dir = scratch.0.join("admin_cells");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join(admin_filename(4)),
        cell_format::write::write_cell(
            0,
            0,
            &[(
                TAG_ADMN,
                &[
                    tests::square(10, 4, 0., 6.),
                    tests::square(10, 4, 2., 3.),
                    tests::square(11, 4, 7., 9.),
                ],
            )],
        ),
    )
    .unwrap();
    let data = load_from_dirs(std::slice::from_ref(&scratch.0), &[4]);
    let mut model = crate::model::AppModel::new();
    model.selected_root = Some(scratch.0.clone());
    model.show_admin = true;
    model.globe_view.local_mode = true;
    model.admin_options.enabled = [false; 10];
    model.admin_options.enabled[2] = true;
    model.admin_options.opacity = 0.3;
    let adapter = block_on(wgpu::Instance::default().request_adapter(&Default::default()))
        .expect("GPU adapter");
    let (device, queue) = block_on(adapter.request_device(&Default::default(), None)).unwrap();
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut renderer = egui_wgpu::Renderer::new(&device, format, None, 1, false);
    let ctx = egui::Context::default();
    crate::theme::install(&ctx);
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1024., 768.));
    let map = egui::Rect::from_min_max(egui::pos2(0., 0.), egui::pos2(780., 768.));
    let screen = egui_wgpu::ScreenDescriptor {
        size_in_pixels: [1024, 768],
        pixels_per_point: 1.,
    };
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("admin QA"),
        size: wgpu::Extent3d {
            width: 1024,
            height: 768,
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
    for shade in [false, true, false] {
        model.admin_options.shade = shade;
        for frame in 0..3 {
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(rect),
                    ..Default::default()
                },
                |ctx| {
                    egui::SidePanel::right("admin-qa-controls")
                        .exact_width(230.)
                        .resizable(false)
                        .show(ctx, |ui| {
                            ui.checkbox(&mut model.show_admin, "Admin Boundaries");
                            super::super::admin_controls::controls(ui, &mut model);
                        });
                    let painter = ctx
                        .layer_painter(egui::LayerId::background())
                        .with_clip_rect(map);
                    painter.text(
                        egui::pos2(40., 60.),
                        egui::Align2::LEFT_TOP,
                        "Administrative boundaries · synthetic QA geometry",
                        egui::FontId::proportional(18.),
                        egui::Color32::WHITE,
                    );
                    super::super::admin_render::paint(&painter, &data, shade, 0.3, 0., |p| {
                        egui::pos2(70. + p.lon * 65., 640. - p.lat * 55.)
                    });
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
                                a: 1.,
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
                size: 1024 * 768 * 4,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            if frame == 2 {
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
            if frame == 2 {
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
    let pixel =
        |i: usize, x: usize, y: usize| &pictures[i][(y * 1024 + x) * 4..(y * 1024 + x + 1) * 4];
    assert_ne!(
        pixel(0, 135, 585),
        pixel(1, 135, 585),
        "region interior shades"
    );
    assert_eq!(
        pixel(0, 232, 502),
        pixel(1, 232, 502),
        "hole remains unshaded"
    );
    for y in 0..768 {
        assert_eq!(
            &pictures[0][y * 4096..y * 4096 + 780 * 4],
            &pictures[2][y * 4096..y * 4096 + 780 * 4]
        );
    }
    if let Ok(path) = std::env::var("ONEKEE_ADMIN_PREVIEW") {
        image::save_buffer(path, &pictures[1], 1024, 768, image::ColorType::Rgba8).unwrap();
    }
}
