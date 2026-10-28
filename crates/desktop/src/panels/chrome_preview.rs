//! Optional GPU rendering of real chrome for visual QA.
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
#[ignore = "requires GPU; optional ONEKEE_CHROME_PREVIEW output directory"]
fn chrome_gpu_preview() {
    let adapter = block_on(wgpu::Instance::default().request_adapter(&Default::default()))
        .expect("GPU adapter");
    let (device, queue) = block_on(adapter.request_device(&Default::default(), None)).unwrap();
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("chrome QA"),
        size: wgpu::Extent3d {
            width: 1536,
            height: 864,
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
        size_in_pixels: [1536, 864],
        pixels_per_point: 1.,
    };
    for (name, section, drawer) in [
        ("rail", None, false),
        ("settings", Some(Section::Overview), false),
        ("terrain", Some(Section::Terrain), false),
        ("appearance", Some(Section::Appearance), false),
        ("sources", Some(Section::Sources), false),
        ("paths", Some(Section::Paths), false),
        ("activity", Some(Section::Activity), false),
        ("layers", None, true),
    ] {
        let ctx = context();
        let mut model = model();
        model.show_layer_drawer = drawer;
        model.settings_open = section.is_some();
        if let Some(section) = section {
            ctx.data_mut(|d| d.insert_temp(egui::Id::new("settings-section"), section));
        }
        let mut renderer = egui_wgpu::Renderer::new(&device, format, None, 1, false);
        for index in 0..3 {
            let output = frame(&ctx, &mut model, 1536., vec![]);
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
                size: 1536 * 864 * 4,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            if index == 2 {
                encoder.copy_texture_to_buffer(
                    target.as_image_copy(),
                    wgpu::TexelCopyBufferInfo {
                        buffer: &readback,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(6144),
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
            if index == 2 {
                let (tx, rx) = std::sync::mpsc::channel();
                readback
                    .slice(..)
                    .map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
                device.poll(wgpu::Maintain::Wait);
                rx.recv().unwrap().unwrap();
                if let Ok(dir) = std::env::var("ONEKEE_CHROME_PREVIEW") {
                    std::fs::create_dir_all(&dir).unwrap();
                    image::save_buffer(
                        std::path::Path::new(&dir).join(format!("{name}.png")),
                        &readback.slice(..).get_mapped_range(),
                        1536,
                        864,
                        image::ColorType::Rgba8,
                    )
                    .unwrap();
                }
            }
        }
    }
}
