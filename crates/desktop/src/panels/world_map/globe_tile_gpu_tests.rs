//! Hardware regressions and opt-in real native contour timing.
use super::super::super::{
    contour_asset::residency,
    globe_scene::{GlobeLayout, globe_layout},
    local_contour_pass::LocalTileId,
};
use super::*;
use std::future::Future;
use std::task::{Context, Poll, Wake, Waker};
use std::time::Instant;
fn block_on<T>(future: impl Future<Output = T>) -> T {
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
pub(super) fn gpu() -> (wgpu::Device, wgpu::Queue) {
    let instance = wgpu::Instance::default();
    let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
        .expect("working GPU");
    eprintln!("Adapter: {:?}", adapter.get_info());
    block_on(adapter.request_device(&wgpu::DeviceDescriptor::default(), None)).unwrap()
}
pub(super) fn target(device: &wgpu::Device, size: [u32; 2]) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("private contour regression target"),
        size: wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}
pub(super) fn draw(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    res: &ContourPassResources,
    texture: &wgpu::Texture,
    chunks: &[(&Chunk, &InstanceChunk)],
    vertices: u32,
    viewport: Option<Viewport>,
    interval: i32,
) {
    let view = texture.create_view(&Default::default());
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
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
        pass.set_pipeline(&res.pipeline);
        pass.set_bind_group(
            0,
            &res.bind_group,
            &[ContourLayer::SrtmGlobe.slot() * UNIFORM_STRIDE as u32],
        );
        for (chunk, gpu) in chunks {
            if viewport.is_some_and(|v| !v.intersects(chunk.bounds)) {
                continue;
            }
            pass.set_vertex_buffer(0, gpu.buffer.slice(..));
            density::ranges(&chunk.planes, gpu.count, interval, |range| {
                pass.draw(0..vertices, range)
            });
        }
    }
    queue.submit([encoder.finish()]);
    device.poll(wgpu::Maintain::Wait);
}
pub(super) fn pixels(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
) -> Vec<u8> {
    let size = texture.size();
    let row = (size.width * 4).div_ceil(256) * 256;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(row * size.height),
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
                bytes_per_row: Some(row),
                rows_per_image: Some(size.height),
            },
        },
        size,
    );
    queue.submit([encoder.finish()]);
    let (tx, rx) = std::sync::mpsc::channel();
    buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
        tx.send(r).unwrap();
    });
    device.poll(wgpu::Maintain::Wait);
    rx.recv().unwrap().unwrap();
    let result = buffer.slice(..).get_mapped_range().to_vec();
    buffer.unmap();
    result
}
fn old_resources(device: &wgpu::Device) -> ContourPassResources {
    let shader=CONTOUR_WGSL.replace("case 2u: { t = 0.0; side = 1.0; }\n        default: { t = 1.0; side = 1.0; }", "case 2u: { t = 1.0; side = 1.0; }\n        case 3u: { t = 0.0; side = -1.0; }\n        case 4u: { t = 1.0; side = 1.0; }\n        default: { t = 0.0; side = 1.0; }");
    assert_ne!(shader, CONTOUR_WGSL);
    ContourPassResources::with_shader(
        device,
        wgpu::TextureFormat::Rgba8Unorm,
        &shader,
        wgpu::PrimitiveTopology::TriangleList,
    )
}
pub(super) fn set_uniforms(
    queue: &wgpu::Queue,
    res: &ContourPassResources,
    layout: &GlobeLayout,
    view: &GlobeViewState,
    rect: egui::Rect,
) {
    let mut u = ContourCallback::new(
        ContourLayer::SrtmGlobe,
        0,
        Arc::new(vec![]),
        layout,
        view,
        0.02,
        0.92,
        1.,
        1.,
    )
    .uniforms;
    u.viewport_size = [rect.width(), rect.height()];
    queue.write_buffer(
        &res.uniform_buf,
        ContourLayer::SrtmGlobe.slot() as u64 * UNIFORM_STRIDE,
        bytemuck::bytes_of(&u),
    );
}
#[test]
#[ignore = "requires working GPU; private offscreen render and staging regression"]
fn bounded_uploads_publish_atomically_and_strip_matches_previous_pixels() {
    let (device, queue) = gpu();
    let mut gpu = Gpu::default();
    let small = Arc::new(prepare(
        super::tests::frame(vec![super::tests::tile(0., 200)]),
        None,
        egui::Color32::WHITE,
        egui::Color32::BLACK,
    ));
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(256., 128.));
    let view = GlobeViewState::from_focus(GeoPoint { lat: 0., lon: 0. });
    let layout = GlobeLayout {
        center: rect.center(),
        radius: 50000.,
        focal_length: 2.,
        camera_distance: 3.,
    };
    let viewport = Viewport::new(&layout, &view, rect);
    assert!(gpu.stage(&device, &small, viewport));
    let dense = super::tests::tile(-30., 800_000);
    let large = Arc::new(prepare(
        super::tests::frame(vec![super::tests::tile(100., 200), dense]),
        Some(&small),
        egui::Color32::WHITE,
        egui::Color32::BLACK,
    ));
    let mut frames = 0;
    loop {
        let before: u64 = gpu.tiles.values().flatten().map(|c| c.buffer.size()).sum();
        let ready = gpu.stage(&device, &large, viewport);
        let after: u64 = gpu.tiles.values().flatten().map(|c| c.buffer.size()).sum();
        assert!(after.saturating_sub(before) <= UPLOAD_BYTES_PER_FRAME as u64);
        frames += 1;
        if ready {
            break;
        }
        assert!(Arc::ptr_eq(gpu.displayed.as_ref().unwrap(), &small));
        assert!(frames < 100);
    }
    assert!(frames > 1);
    assert!(Arc::ptr_eq(gpu.displayed.as_ref().unwrap(), &large));
    assert!(!gpu.tiles.contains_key(&small.tiles[0].version));
    let old = old_resources(&device);
    let new = ContourPassResources::new(&device, wgpu::TextureFormat::Rgba8Unorm);
    set_uniforms(&queue, &old, &layout, &view, rect);
    set_uniforms(&queue, &new, &layout, &view, rect);
    let texture = target(&device, [256, 128]);
    let chunks: Vec<_> = large
        .tiles
        .iter()
        .flat_map(|t| t.chunks.iter().zip(&gpu.tiles[&t.version]))
        .collect();
    draw(&device, &queue, &old, &texture, &chunks, 6, None, 1);
    let before = pixels(&device, &queue, &texture);
    draw(
        &device,
        &queue,
        &new,
        &texture,
        &chunks,
        4,
        Some(viewport),
        1,
    );
    let after = pixels(&device, &queue, &texture);
    assert!(before.chunks(4).any(|p| p[0] > 0), "nonempty contour image");
    let differing = before
        .iter()
        .zip(&after)
        .filter(|(a, b)| a.abs_diff(**b) > 1)
        .count();
    assert!(
        differing < before.len() / 1000,
        "quad/culling mismatch: {differing} channels"
    );
    callback_lifecycle(&device, &queue, small, &layout, &view, rect);
}

fn callback_lifecycle(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    candidate: Arc<Prepared>,
    layout: &GlobeLayout,
    view: &GlobeViewState,
    rect: egui::Rect,
) {
    let mut renderer =
        egui_wgpu::Renderer::new(device, wgpu::TextureFormat::Rgba8Unorm, None, 1, false);
    renderer
        .callback_resources
        .insert(ContourPassResources::new(
            device,
            wgpu::TextureFormat::Rgba8Unorm,
        ));
    let ctx = egui::Context::default();
    let screen = egui_wgpu::ScreenDescriptor {
        size_in_pixels: [256, 128],
        pixels_per_point: 1.0,
    };
    for shown in [true, false, true] {
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(rect),
                ..Default::default()
            },
            |ctx| {
                let painter = ctx.layer_painter(egui::LayerId::new(
                    egui::Order::Foreground,
                    egui::Id::new("native-lifecycle"),
                ));
                painter.add(residency_callback(rect));
                if shown {
                    let mut uniforms = ContourCallback::new(
                        ContourLayer::SrtmGlobe,
                        0,
                        Arc::new(vec![]),
                        layout,
                        view,
                        0.02,
                        0.92,
                        1.,
                        1.,
                    )
                    .uniforms;
                    uniforms.viewport_size = [rect.width(), rect.height()];
                    painter.add(egui_wgpu::Callback::new_paint_callback(
                        rect,
                        Callback {
                            candidate: candidate.clone(),
                            uniforms,
                            viewport: Viewport::new(layout, view, rect),
                            ctx: ctx.clone(),
                        },
                    ));
                }
            },
        );
        let jobs = ctx.tessellate(output.shapes, output.pixels_per_point);
        let mut encoder = device.create_command_encoder(&Default::default());
        let commands = renderer.update_buffers(device, queue, &mut encoder, &jobs, &screen);
        queue.submit(
            commands
                .into_iter()
                .chain(std::iter::once(encoder.finish())),
        );
        device.poll(wgpu::Maintain::Wait);
        let resources = renderer
            .callback_resources
            .get::<ContourPassResources>()
            .unwrap();
        assert_eq!(resources.terrain.displayed.is_some(), shown);
        assert_eq!(resources.terrain.tiles.is_empty(), !shown);
    }
}

#[test]
#[ignore = "read-only native cache benchmark; set ONEKEE_NATIVE_BENCH_DB; requires GPU"]
fn benchmark_native_globe_tiles() {
    let path = std::env::var_os("ONEKEE_NATIVE_BENCH_DB").expect("explicit native DB path");
    let db = super::super::super::srtm_focus_cache::db::open_cache_db_read_only(
        std::path::Path::new(&path),
    )
    .unwrap();
    let mut tiles = Vec::new();
    // Actual installed native Yemen/Red Sea window; include retained neighbors.
    for lat in 7..=12 {
        for lon in 24..=31 {
            let mut stmt=db.prepare("SELECT elevation_m,geom FROM contour_tiles WHERE zoom_bucket=0 AND lat_bucket=?1 AND lon_bucket=?2 ORDER BY fid").unwrap();
            let mut rows = stmt.query(rusqlite::params![lat, lon]).unwrap();
            let mut contours = Vec::new();
            while let Some(row) = rows.next().unwrap() {
                let elevation_m = row.get(0).unwrap();
                let bytes = row.get_ref(1).unwrap().as_blob().unwrap();
                for points in
                    tile_archive::gpkg::parse_gpkg_lines(bytes, |lon, lat| GeoPoint { lon, lat })
                {
                    contours.push(ContourPath {
                        elevation_m,
                        points,
                    });
                }
            }
            if !contours.is_empty() {
                contours.sort_by(|a, b| a.elevation_m.abs().total_cmp(&b.elevation_m.abs()));
                tiles.push(LocalTileGeometry {
                    id: LocalTileId {
                        zoom_bucket: 0,
                        lat_bucket: lat,
                        lon_bucket: lon,
                    },
                    bounds: residency::Bounds::from_contours(&contours),
                    contours: Arc::new(contours),
                });
            }
        }
    }
    assert!(!tiles.is_empty());
    let source = super::tests::frame(tiles);
    let start = Instant::now();
    let flat: Vec<_> = source
        .tiles
        .iter()
        .flat_map(|t| t.contours.iter().cloned())
        .collect();
    let baseline: Vec<SegmentInstance> = flat
        .par_iter()
        .flat_map_iter(|c| {
            let color = linear_u8(egui::Color32::from_rgb(234, 58, 12));
            c.points.windows(2).map(move |p| SegmentInstance {
                a: unit_vec(p[0]),
                b: unit_vec(p[1]),
                color,
            })
        })
        .collect();
    eprintln!(
        "previous full merge + instance rebuild: {:.2}ms",
        start.elapsed().as_secs_f64() * 1000.
    );
    drop(flat);

    let total: usize = source
        .tiles
        .iter()
        .flat_map(|t| t.contours.iter())
        .map(|c| c.points.len().saturating_sub(1))
        .sum();
    eprintln!(
        "NATIVE BENCH: {} actual tiles, {total} unchanged segments",
        source.tiles.len()
    );
    let start = Instant::now();
    let prepared = Arc::new(prepare(
        source.clone(),
        None,
        egui::Color32::from_rgb(234, 58, 12),
        egui::Color32::WHITE,
    ));
    eprintln!(
        "initial spatial instance build: {:.2}ms",
        start.elapsed().as_secs_f64() * 1000.
    );
    let mut changed = source.tiles.clone();
    changed.pop();
    let start = Instant::now();
    let reused = prepare(
        super::tests::frame(changed),
        Some(&prepared),
        egui::Color32::from_rgb(234, 58, 12),
        egui::Color32::WHITE,
    );
    eprintln!(
        "tile-set update reusing {} tiles: {:.3}ms",
        reused.tiles.len(),
        start.elapsed().as_secs_f64() * 1000.
    );
    assert!(
        reused
            .tiles
            .iter()
            .zip(&prepared.tiles)
            .all(|(a, b)| Arc::ptr_eq(a, b))
    );
    let (device, queue) = gpu();
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1920., 1080.));
    let mut view = GlobeViewState::from_focus(GeoPoint { lat: 15., lon: 45. });
    view.zoom = 24.;
    let layout = globe_layout(rect, &view);
    let viewport = Viewport::new(&layout, &view, rect);
    let start = Instant::now();
    let baseline_gpu = split_instance_buffers(&device, "old whole layer", &baseline);
    eprintln!(
        "previous synchronous whole-layer upload: {:.2}ms",
        start.elapsed().as_secs_f64() * 1000.
    );
    drop(baseline);
    let dummy = Chunk {
        bounds: Bounds {
            min_lon: -180.,
            max_lon: 180.,
            min_lat: -90.,
            max_lat: 90.,
        },
        instances: vec![],
        planes: vec![],
    };
    let baseline_chunks: Vec<_> = baseline_gpu.iter().map(|g| (&dummy, g)).collect();
    let mut gpu = Gpu::default();
    let mut max_ms = 0f64;
    let mut upload_frames = 0;
    loop {
        let start = Instant::now();
        let done = gpu.stage(&device, &prepared, viewport);
        max_ms = max_ms.max(start.elapsed().as_secs_f64() * 1000.);
        upload_frames += 1;
        if done {
            break;
        }
    }
    eprintln!("staged uploads: {upload_frames} frames, max CPU submission {max_ms:.2}ms");
    gpu.update_density(viewport);
    let interval = gpu.density.interval_m;
    eprintln!("wide-view whole-plane spacing: {interval}m");
    let chunks: Vec<_> = prepared
        .tiles
        .iter()
        .flat_map(|t| t.chunks.iter().zip(&gpu.tiles[&t.version]))
        .collect();
    let visible: usize = chunks
        .iter()
        .filter(|(c, _)| viewport.intersects(c.bounds))
        .map(|(c, _)| density::count(&c.planes, interval))
        .sum();
    let old = old_resources(&device);
    let new = ContourPassResources::new(&device, wgpu::TextureFormat::Rgba8Unorm);
    set_uniforms(&queue, &old, &layout, &view, rect);
    set_uniforms(&queue, &new, &layout, &view, rect);
    let texture = target(&device, [1920, 1080]);
    for _ in 0..3 {
        draw(
            &device,
            &queue,
            &old,
            &texture,
            &baseline_chunks,
            6,
            None,
            1,
        );
        draw(
            &device,
            &queue,
            &new,
            &texture,
            &chunks,
            4,
            Some(viewport),
            interval,
        );
    }
    let mut timings = [Vec::new(), Vec::new()];
    for _ in 0..10 {
        for (i, res, vertices, cull) in [(0, &old, 6, None), (1, &new, 4, Some(viewport))] {
            let start = Instant::now();
            draw(
                &device,
                &queue,
                res,
                &texture,
                if i == 0 { &baseline_chunks } else { &chunks },
                vertices,
                cull,
                if i == 0 { 1 } else { interval },
            );
            timings[i].push(start.elapsed().as_secs_f64() * 1000.);
        }
    }
    for values in &mut timings {
        values.sort_by(f64::total_cmp);
    }
    eprintln!(
        "NATIVE DRAW: {total} -> {visible} segments submitted; old median {:.2}ms; new median {:.2}ms (submit + GPU wait)",
        timings[0][5], timings[1][5]
    );
    let mut cached = texture::Cache::new(&device, new.format);
    let mut uniforms = ContourCallback::new(
        ContourLayer::SrtmGlobe,
        0,
        Arc::new(vec![]),
        &layout,
        &view,
        0.02,
        0.92,
        1.,
        1.,
    )
    .uniforms;
    uniforms.viewport_size = [1920., 1080.];
    let mut refinement = Vec::new();
    loop {
        let start = Instant::now();
        let mut encoder = device.create_command_encoder(&Default::default());
        let pending = cached.prepare(
            &device,
            &queue,
            &mut encoder,
            new.format,
            &new.pipeline,
            &new.bind_group,
            &prepared,
            &gpu.tiles,
            &uniforms,
            viewport,
            interval,
        );
        queue.submit([encoder.finish()]);
        device.poll(wgpu::Maintain::Wait);
        refinement.push(start.elapsed().as_secs_f64() * 1000.);
        if !pending {
            break;
        }
        assert!(refinement.len() < 100);
    }
    let render_cached = |cached: &texture::Cache| {
        let target_view = texture.create_view(&Default::default());
        let mut encoder = device.create_command_encoder(&Default::default());
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
            cached.paint(&mut pass.forget_lifetime());
        }
        queue.submit([encoder.finish()]);
        device.poll(wgpu::Maintain::Wait);
    };
    let mut stable = Vec::new();
    for _ in 0..20 {
        let start = Instant::now();
        render_cached(&cached);
        stable.push(start.elapsed().as_secs_f64() * 1000.);
    }
    stable.sort_by(f64::total_cmp);
    eprintln!(
        "FULL DETAIL CACHE: {} refinement frames; median stationary draw {:.2}ms; worst refinement {:.2}ms",
        refinement.len(),
        stable[10],
        refinement.iter().copied().fold(0f64, f64::max)
    );
    let mut moving = Vec::new();
    for i in 0..45 {
        let start = Instant::now();
        let yaw = view.yaw + 0.00015 * (i + 1) as f32;
        uniforms.yaw_sin = yaw.sin(); uniforms.yaw_cos = yaw.cos();
        queue.write_buffer(&new.uniform_buf, ContourLayer::SrtmGlobe.slot() as u64 * UNIFORM_STRIDE, bytemuck::bytes_of(&uniforms));
        let mut encoder = device.create_command_encoder(&Default::default());
        cached.prepare(&device, &queue, &mut encoder, new.format, &new.pipeline, &new.bind_group, &prepared, &gpu.tiles, &uniforms, viewport, interval);
        queue.submit([encoder.finish()]);
        render_cached(&cached);
        moving.push(start.elapsed().as_secs_f64() * 1000.);
    }
    moving.sort_by(f64::total_cmp);
    eprintln!("RETAINED FULL DETAIL PAN: median {:.2}ms; worst {:.2}ms (preview + refinement + reprojection + GPU wait)", moving[22], moving[44]);
    if let Some(path) = std::env::var_os("ONEKEE_NATIVE_BENCH_IMAGE") {
        image::save_buffer(
            path,
            &pixels(&device, &queue, &texture),
            1920,
            1080,
            image::ColorType::Rgba8,
        )
        .unwrap();
    }
}
