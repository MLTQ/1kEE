//! Native GPU preview with inert camera fixtures; no live source calls.
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
            Poll::Ready(v) => return v,
            Poll::Pending => std::thread::park_timeout(std::time::Duration::from_millis(10)),
        }
    }
}
#[test]
#[ignore = "requires GPU; optional ONEKEE_EVENT_CAMERA_PREVIEW output directory"]
fn event_camera_sidebar_gpu_preview() {
    let adapter = block_on(wgpu::Instance::default().request_adapter(&Default::default()))
        .expect("GPU adapter");
    let (device, queue) = block_on(adapter.request_device(&Default::default(), None)).unwrap();
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("camera sidebar QA"),
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
    let target_view = target.create_view(&Default::default());
    let screen = egui_wgpu::ScreenDescriptor {
        size_in_pixels: [1280, 720],
        pixels_per_point: 1.,
    };
    for (name, obs) in [("camera-sidebar", false), ("camera-sidebar-obs", true)] {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut model = model();
        model.obs_view = obs;
        let mut sidebar = EventCameraSidebar::default();
        prime(&ctx, &mut sidebar, &model);
        let first = &sidebar.cameras[0].camera;
        sidebar.players.insert(
            first.id.clone(),
            CameraFeedPlayer::fixture(
                &ctx,
                first,
                egui::ColorImage::new([640, 480], egui::Color32::from_rgb(45, 70, 90)),
            )
            .0,
        );
        fail_second_feed(&mut sidebar);
        let mut renderer = egui_wgpu::Renderer::new(&device, format, None, 1, false);
        for index in 0..4 {
            let (output, _) = frame(&ctx, &mut sidebar, &mut model, 1280.0);
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
            if index == 3 {
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
            if index == 3 {
                let (tx, rx) = std::sync::mpsc::channel();
                readback
                    .slice(..)
                    .map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
                device.poll(wgpu::Maintain::Wait);
                rx.recv().unwrap().unwrap();
                {
                    let dir = std::env::var("ONEKEE_EVENT_CAMERA_PREVIEW")
                        .unwrap_or_else(|_| "target/event-camera-qa".into());
                    std::fs::create_dir_all(&dir).unwrap();
                    image::save_buffer(
                        std::path::Path::new(&dir).join(format!("{name}.png")),
                        &readback.slice(..).get_mapped_range(),
                        1280,
                        720,
                        image::ColorType::Rgba8,
                    )
                    .unwrap();
                }
            }
        }
    }
}
