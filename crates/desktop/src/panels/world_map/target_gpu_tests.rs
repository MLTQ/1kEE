use super::*;
use std::{
    future::Future,
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
    let mut future = std::pin::pin!(future);
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(v) => return v,
            Poll::Pending => std::thread::park_timeout(std::time::Duration::from_millis(10)),
        }
    }
}
fn gpu() -> (wgpu::Device, wgpu::Queue) {
    let instance = wgpu::Instance::default();
    let adapter = block_on(instance.request_adapter(&Default::default())).expect("Target test GPU");
    eprintln!("Target GPU: {:?}", adapter.get_info());
    block_on(adapter.request_device(&wgpu::DeviceDescriptor::default(), None)).unwrap()
}
fn pixels(resources: &Resources, device: &wgpu::Device, queue: &wgpu::Queue) -> Vec<u8> {
    let texture = resources.texture.as_ref().unwrap();
    read_texture(&texture._color, texture.size, device, queue)
}
fn read_texture(
    texture: &wgpu::Texture,
    [width, height]: [u32; 2],
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> Vec<u8> {
    let stride = (width * 4).div_ceil(256) * 256;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(stride) * u64::from(height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: Default::default(),
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(stride),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);
    let (tx, rx) = std::sync::mpsc::channel();
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| tx.send(result).unwrap());
    device.poll(wgpu::Maintain::Wait);
    rx.recv().unwrap().unwrap();
    let data = buffer.slice(..).get_mapped_range();
    data.chunks_exact(stride as usize)
        .flat_map(|r| r[..width as usize * 4].iter().copied())
        .collect()
}
fn render(
    resources: &mut Resources,
    draw: &Draw,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> Vec<u8> {
    let command = resources.prepare(draw, device, queue);
    queue.submit([command]);
    device.poll(wgpu::Maintain::Wait);
    pixels(resources, device, queue)
}
fn draw(ctx: egui::Context) -> Draw {
    Draw {
        session: 1,
        camera: Camera {
            eye: [0., -10., 0.],
            yaw: 0.,
            pitch: 0.,
            ..Default::default()
        },
        radius: 1000.,
        clouds: vec![],
        terrain: None,
        context: None,
        points: true,
        surface: false,
        context_visible: false,
        class_mask: u32::MAX,
        color_mode: 0,
        point_size: 5.,
        rect: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(64., 64.)),
        ctx,
    }
}
#[test]
#[ignore = "requires a GPU; checks actual depth rendering, filtering, flight and resource release"]
fn target_gpu_preserves_depth_filters_and_releases_buffers() {
    let (device, queue) = gpu();
    let mut resources = Resources::new(&device, wgpu::TextureFormat::Rgba8Unorm);
    let mut draw = draw(egui::Context::default());
    let vertex = |position, class| Vertex {
        position,
        color: [255; 4],
        class,
    };
    draw.clouds = vec![Arc::new(Batch {
        id: 11,
        points: vec![vertex([0., 0., 0.], 2), vertex([0., 10., 0.], 6)],
    })];
    let first = render(&mut resources, &draw, &device, &queue);
    let center = (32 * 64 + 32) * 4;
    let ground = first[center..center + 3].to_vec();
    draw.clouds = vec![Arc::new(Batch {
        id: 12,
        points: vec![vertex([0., 10., 0.], 6), vertex([0., 0., 0.], 2)],
    })];
    let reversed = render(&mut resources, &draw, &device, &queue);
    assert_eq!(&reversed[center..center + 3], ground);
    draw.class_mask = 1 << 6;
    let buildings = render(&mut resources, &draw, &device, &queue);
    assert_ne!(&buildings[center..center + 3], ground);
    draw.camera.eye[0] = 8.;
    let moved = render(&mut resources, &draw, &device, &queue);
    assert_ne!(&moved[center..center + 3], &buildings[center..center + 3]);
    assert!(!resources.batches.is_empty());
    // Use the real egui release callback on return to Local.
    let ctx = egui::Context::default();
    let mut renderer =
        egui_wgpu::Renderer::new(&device, wgpu::TextureFormat::Rgba8Unorm, None, 1, false);
    renderer.callback_resources.insert(resources);
    draw.camera.eye[0] = 0.;
    draw.class_mask = u32::MAX;
    let frame = ctx.run(
        egui::RawInput {
            screen_rect: Some(draw.rect),
            ..Default::default()
        },
        |ctx| {
            ctx.layer_painter(egui::LayerId::background()).add(
                egui_wgpu::Callback::new_paint_callback(draw.rect, draw.clone()),
            );
        },
    );
    let jobs = ctx.tessellate(frame.shapes, 1.);
    let mut encoder = device.create_command_encoder(&Default::default());
    let screen = egui_wgpu::ScreenDescriptor {
        size_in_pixels: [64, 64],
        pixels_per_point: 1.,
    };
    let commands = renderer.update_buffers(&device, &queue, &mut encoder, &jobs, &screen);
    let output = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Target composite test"),
        size: wgpu::Extent3d {
            width: 64,
            height: 64,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = output.create_view(&Default::default());
    {
        let mut pass = encoder
            .begin_render_pass(&wgpu::RenderPassDescriptor {
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
            })
            .forget_lifetime();
        renderer.render(&mut pass, &jobs, &screen);
    }
    queue.submit(
        commands
            .into_iter()
            .chain(std::iter::once(encoder.finish())),
    );
    device.poll(wgpu::Maintain::Wait);
    let composite = read_texture(&output, [64, 64], &device, &queue);
    for (actual, expected) in composite[center..center + 3].iter().zip(&ground) {
        assert!(
            (i16::from(*actual) - i16::from(*expected)).abs() <= 2,
            "composite must retain orientation and sRGB colors"
        );
    }
    let frame = ctx.run(
        egui::RawInput {
            screen_rect: Some(draw.rect),
            ..Default::default()
        },
        |ctx| {
            ctx.layer_painter(egui::LayerId::background())
                .add(release(draw.rect));
        },
    );
    let jobs = ctx.tessellate(frame.shapes, 1.);
    let mut encoder = device.create_command_encoder(&Default::default());
    let commands = renderer.update_buffers(
        &device,
        &queue,
        &mut encoder,
        &jobs,
        &egui_wgpu::ScreenDescriptor {
            size_in_pixels: [64, 64],
            pixels_per_point: 1.,
        },
    );
    queue.submit(
        commands
            .into_iter()
            .chain(std::iter::once(encoder.finish())),
    );
    device.poll(wgpu::Maintain::Wait);
    let resource = renderer.callback_resources.get::<Resources>().unwrap();
    assert!(
        resource.batches.is_empty() && resource.texture.is_none() && resource.indices.is_none()
    );
}

#[test]
#[ignore = "requires live NC source data, writable LiDAR cache and a GPU; writes benchmark and preview to /tmp"]
fn target_live_nc_cutout_benchmark() {
    use crate::target_mode::{ContextInput, Session};
    use lidar_pipeline::region::Region;
    let catalog = std::env::var_os("ONEKEE_TARGET_TEST_CATALOG")
        .map(std::path::PathBuf::from)
        .expect("ONEKEE_TARGET_TEST_CATALOG");
    let (device, queue) = gpu();
    let ctx = egui::Context::default();
    let region = Region {
        lon: -77.58,
        lat: 35.26,
        radius_m: 1000.,
    };
    let start = std::time::Instant::now();
    let asset_root =
        std::env::var_os("ONEKEE_TARGET_TEST_ASSET_ROOT").map(std::path::PathBuf::from);
    let mut session =
        Session::new(region, catalog, asset_root, ContextInput::default(), &ctx).unwrap();
    let mut last = std::time::Instant::now();
    while session.loading() && start.elapsed() < std::time::Duration::from_secs(230) {
        session.poll(&ctx);
        if last.elapsed() > std::time::Duration::from_secs(10) {
            eprintln!("Target: {} · {}", session.status, session.terrain_status);
            last = std::time::Instant::now();
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let load_ms = start.elapsed().as_secs_f64() * 1000.;
    assert!(session.points > 10_000, "{}", session.status);
    assert!(session.terrain.is_some(), "{}", session.terrain_status);
    assert!(
        session
            .clouds
            .iter()
            .flat_map(|b| &b.points)
            .all(|p| p.position[0].hypot(p.position[1]) <= 1000.001)
    );
    let z = session
        .clouds
        .iter()
        .flat_map(|b| &b.points)
        .map(|p| p.position[2])
        .fold([f32::INFINITY, f32::NEG_INFINITY], |a, z| {
            [a[0].min(z), a[1].max(z)]
        });
    assert!(
        z[1] - z[0] > 1.,
        "cloud must retain real vertical structure"
    );
    let mut draw = Draw {
        session: session.id,
        camera: session.camera.clone(),
        radius: 1000.,
        clouds: session.clouds.clone(),
        terrain: session.terrain.clone(),
        context: None,
        points: true,
        surface: false,
        context_visible: false,
        class_mask: u32::MAX,
        color_mode: 0,
        point_size: 1.2,
        rect: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280., 720.)),
        ctx: ctx.clone(),
    };
    let mut resources = Resources::new(&device, wgpu::TextureFormat::Rgba8Unorm);
    for _ in 0..160 {
        let command = resources.prepare(&draw, &device, &queue);
        queue.submit([command]);
        device.poll(wgpu::Maintain::Wait);
        if draw.clouds.iter().all(|b| {
            resources
                .batches
                .get(&b.id)
                .is_some_and(|g| g.uploaded == b.points.len())
        }) {
            break;
        }
    }
    let start = std::time::Instant::now();
    for _ in 0..30 {
        let command = resources.prepare(&draw, &device, &queue);
        queue.submit([command]);
        device.poll(wgpu::Maintain::Wait);
    }
    let frame_ms = start.elapsed().as_secs_f64() * 1000. / 30.;
    let image = render(&mut resources, &draw, &device, &queue);
    let mut encoder = png::Encoder::new(
        std::fs::File::create("/tmp/1kee-target-nc-points.png").unwrap(),
        1280,
        720,
    );
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .unwrap()
        .write_image_data(&image)
        .unwrap();
    draw.points = false;
    draw.surface = true;
    let start = std::time::Instant::now();
    for _ in 0..30 {
        let command = resources.prepare(&draw, &device, &queue);
        queue.submit([command]);
        device.poll(wgpu::Maintain::Wait);
    }
    let dem_ms = start.elapsed().as_secs_f64() * 1000. / 30.;
    let dem = render(&mut resources, &draw, &device, &queue);
    assert_ne!(
        image, dem,
        "point and DEM modes must show distinct measured geometry"
    );
    let mut encoder = png::Encoder::new(
        std::fs::File::create("/tmp/1kee-target-nc-dem.png").unwrap(),
        1280,
        720,
    );
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .unwrap()
        .write_image_data(&dem)
        .unwrap();
    draw.points = true;
    draw.surface = false;
    draw.camera.eye = [100., -300., 40.];
    draw.camera.yaw = 0.2;
    draw.camera.pitch = -0.12;
    let flight = render(&mut resources, &draw, &device, &queue);
    let mut encoder = png::Encoder::new(
        std::fs::File::create("/tmp/1kee-target-nc-flight.png").unwrap(),
        1280,
        720,
    );
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .unwrap()
        .write_image_data(&flight)
        .unwrap();
    let report = serde_json::json!({"region":{"lat":region.lat,"lon":region.lon,"radius_m":region.radius_m},
        "points":session.points,"load_ms":load_ms,"frame_ms_gpu_synchronized":frame_ms,"point_buffer_bytes":session.points*std::mem::size_of::<Vertex>(),
        "height_range_source_units":z,"summary":session.summary,"status":session.status,"dem":session.terrain_status,
        "dem_frame_ms_gpu_synchronized":dem_ms,"dem_vertex_bytes":session.terrain.as_ref().unwrap().batch.points.len()*std::mem::size_of::<Vertex>(),
        "dem_index_bytes":session.terrain.as_ref().unwrap().indices.len()*4,"context":session.context_status,
        "source":session.source.as_ref().map(|h|serde_json::json!({"id":h.source.id,"date":h.source.date,"crs":h.crs,"publisher":h.publisher})),
        "notes":["Original source coordinates retained; no inferred vertical alignment. Source Z units undeclared in this EPT metadata.",
            "EPT blocks were already cached. Loading time includes a fresh DEM request.",
            "GPU timing includes CPU preparation, submission and a GPU wait per frame; not an interactive FPS claim."]});
    std::fs::write(
        "/tmp/1kee-target-nc-benchmark.json",
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .unwrap();
    eprintln!(
        "TARGET_BENCH points={} load_ms={load_ms:.1} synchronized_frame_ms={frame_ms:.2} status={}",
        session.points, session.status
    );
}
