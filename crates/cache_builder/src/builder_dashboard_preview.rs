//! Optional offscreen visual QA. Writes only a preview image in /tmp.
use super::*;
use eframe::{egui_wgpu, wgpu};
use std::{
    future::Future,
    sync::Arc,
    task::{Context, Poll, Wake, Waker},
};
fn block_on<T>(future: impl Future<Output = T>) -> T {
    struct Unpark(std::thread::Thread);
    impl Wake for Unpark {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = Waker::from(Arc::new(Unpark(std::thread::current())));
    let mut cx = Context::from_waker(&waker);
    let mut f = std::pin::pin!(future);
    loop {
        match f.as_mut().poll(&mut cx) {
            Poll::Ready(v) => return v,
            Poll::Pending => std::thread::park_timeout(Duration::from_millis(10)),
        }
    }
}
#[test]
#[ignore = "requires GPU; writes /tmp/onekee-builder-dashboard.png"]
fn render_dashboard_preview() {
    let mut dashboard = Dashboard::default();
    let mut inventory = Inventory::default();
    for (name, files) in [("road", 22400), ("building", 14200), ("waterway", 9800)] {
        inventory.layers.insert(
            name.into(),
            Layer {
                files,
                bytes: files as u64 * 180000,
                cells: (-50..70).step_by(4).map(|lat| (lat, lat * 2)).collect(),
            },
        );
        inventory.files += files;
        inventory.bytes += files as u64 * 180000;
    }
    inventory.checkpoint =
        "pass1_complete=true\npass1_record_count=650096000\npass2_offset=18200000000".into();
    dashboard.inventory = Some(inventory);
    dashboard.layer = "road".into();
    dashboard.refreshed = Some(Instant::now());
    dashboard.live.start();
    dashboard.live.observe(Some(&build_monitor::Metrics {
        phase: "Way scan".into(),
        scanned_bytes: 19000000000,
        total_bytes: 90000000000,
        checkpoint_bytes: 18200000000,
        ways: 2450000,
        features: 430000,
        cell_writes: 18000,
        ..Default::default()
    }));
    let instance = wgpu::Instance::default();
    let adapter =
        block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).unwrap();
    let (device, queue) =
        block_on(adapter.request_device(&wgpu::DeviceDescriptor::default(), None)).unwrap();
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut renderer = egui_wgpu::Renderer::new(&device, format, None, 1, false);
    let ctx = egui::Context::default();
    ctx.set_visuals(egui::Visuals::dark());
    let width = std::env::var("ONEKEE_PREVIEW_WIDTH")
        .ok()
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(1024);
    assert_eq!(width % 64, 0);
    let height = 1100;
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width as f32, height as f32));
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(rect),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                dashboard.show(
                    ui,
                    "Scanning ways · preview fixture",
                    "Resolving selected ways and merging output cells",
                    true,
                    &[
                        "Node index ready".into(),
                        "Way scan resumed from saved position".into(),
                    ],
                    &[("road", true), ("railway", true)],
                );
            });
        },
    );
    let jobs = ctx.tessellate(output.shapes, output.pixels_per_point);
    for (id, delta) in &output.textures_delta.set {
        renderer.update_texture(&device, &queue, *id, delta);
    }
    let size = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    let screen = egui_wgpu::ScreenDescriptor {
        size_in_pixels: [width, height],
        pixels_per_point: 1.0,
    };
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
    queue.submit(commands.into_iter().chain([encoder.finish()]));
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (width * height * 4) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
        },
        size,
    );
    queue.submit([encoder.finish()]);
    let (tx, rx) = mpsc::channel();
    buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
        tx.send(r).unwrap();
    });
    device.poll(wgpu::Maintain::Wait);
    rx.recv().unwrap().unwrap();
    image::save_buffer(
        "/tmp/onekee-builder-dashboard.png",
        &buffer.slice(..).get_mapped_range(),
        width,
        height,
        image::ColorType::Rgba8,
    )
    .unwrap();
}
