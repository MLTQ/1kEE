//! Real GPU visibility regressions and opt-in repeatable offscreen timing.
use super::*;
use crate::model::GeoPoint;
use std::time::Instant;

fn block_on<T>(future: impl std::future::Future<Output = T>) -> T {
    struct Wake(std::thread::Thread);
    impl std::task::Wake for Wake {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = std::task::Waker::from(Arc::new(Wake(std::thread::current())));
    let mut cx = std::task::Context::from_waker(&waker);
    let mut f = std::pin::pin!(future);
    loop {
        match f.as_mut().poll(&mut cx) {
            std::task::Poll::Ready(x) => return x,
            _ => std::thread::park_timeout(std::time::Duration::from_millis(1)),
        }
    }
}
struct Harness {
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: egui_wgpu::Renderer,
    ctx: egui::Context,
    target: wgpu::Texture,
    size: [u32; 2],
}
impl Harness {
    fn new(size: [u32; 2]) -> Self {
        let instance = wgpu::Instance::default();
        let adapter = block_on(instance.request_adapter(&Default::default())).expect("GPU adapter");
        eprintln!("occlusion GPU: {:?}", adapter.get_info());
        let (device, queue) = block_on(adapter.request_device(&Default::default(), None)).unwrap();
        let mut renderer =
            egui_wgpu::Renderer::new(&device, wgpu::TextureFormat::Rgba8Unorm, None, 1, false);
        renderer
            .callback_resources
            .insert(LocalContourPassResources::new(
                &device,
                wgpu::TextureFormat::Rgba8Unorm,
            ));
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("occlusion regression color"),
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
        });
        Self {
            device,
            queue,
            renderer,
            ctx: Default::default(),
            target,
            size,
        }
    }
    fn render(
        &mut self,
        batches: &[LocalTileBatch],
        p: &LocalProjectionParams,
        on: bool,
        rect: egui::Rect,
        roads: &[LocalTileBatch],
    ) {
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: self.size,
            pixels_per_point: 1.0,
        };
        let full = egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(self.size[0] as f32, self.size[1] as f32),
        );
        let output = self.ctx.run(
            egui::RawInput {
                screen_rect: Some(full),
                ..Default::default()
            },
            |ctx| {
                let painter = ctx
                    .layer_painter(egui::LayerId::background())
                    .with_clip_rect(rect);
                painter.add(residency_callback(rect, ctx));
                if !batches.is_empty() {
                    painter.add(
                        LocalContourCallback::new(
                            LocalContourPass::Surface,
                            batches.to_vec(),
                            p,
                            1.0,
                            1.0,
                            1.0,
                            ctx.clone(),
                        )
                        .with_occlusion(on)
                        .into_paint_callback(rect),
                    );
                }
                if !roads.is_empty() {
                    painter.add(
                        LocalContourCallback::new(
                            LocalContourPass::Roads,
                            roads.to_vec(),
                            p,
                            1.0,
                            2.0,
                            2.0,
                            ctx.clone(),
                        )
                        .into_paint_callback(rect),
                    );
                }
            },
        );
        let jobs = self.ctx.tessellate(output.shapes, output.pixels_per_point);
        let mut encoder = self.device.create_command_encoder(&Default::default());
        let commands =
            self.renderer
                .update_buffers(&self.device, &self.queue, &mut encoder, &jobs, &screen);
        let view = self.target.create_view(&Default::default());
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("occlusion regression"),
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
            self.renderer
                .render(&mut pass.forget_lifetime(), &jobs, &screen);
        }
        self.queue
            .submit(commands.into_iter().chain([encoder.finish()]));
        self.device.poll(wgpu::Maintain::Wait);
    }
    fn pixels(&self) -> Vec<u8> {
        let row = (self.size[0] * 4).div_ceil(256) * 256;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (row * self.size[1]) as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            self.target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: None,
                },
            },
            wgpu::Extent3d {
                width: self.size[0],
                height: self.size[1],
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |x| tx.send(x).unwrap());
        self.device.poll(wgpu::Maintain::Wait);
        rx.recv().unwrap().unwrap();
        let mapped = buffer.slice(..).get_mapped_range();
        let mut pixels = vec![];
        for y in 0..self.size[1] as usize {
            pixels.extend_from_slice(
                &mapped[y * row as usize..y * row as usize + self.size[0] as usize * 4],
            );
        }
        pixels
    }
    fn renders(&self) -> usize {
        self.renderer
            .callback_resources
            .get::<LocalContourPassResources>()
            .unwrap()
            .occlusion
            .renders
    }
}
fn params() -> LocalProjectionParams {
    LocalProjectionParams {
        focus_lon: 0.0,
        focus_lat: 0.0,
        x_factor: 1.0,
        y_factor: 1.0,
        z_factor: 1.0,
        yaw_cos: 1.0,
        yaw_sin: 0.0,
        pitch_cos: 1.0,
        pitch_sin: 0.0,
        focus_center_x: 128.0,
        focus_center_y: 128.0,
        horizontal_scale: 80.0,
        ground_pitch_scale: 40.0,
        ground_depth_scale: 0.0,
        elevation_pitch_scale: 0.0,
        elevation_depth_scale: 80.0,
    }
}
fn batch(paths: Vec<ContourPath>, version: u64) -> LocalTileBatch {
    let instances = paths
        .iter()
        .flat_map(|c| {
            c.points.windows(2).map(move |p| {
                LocalSegmentInstance::line(
                    [p[0].lon, p[0].lat, c.elevation_m],
                    [p[1].lon, p[1].lat, c.elevation_m],
                    egui::Color32::WHITE,
                    false,
                )
            })
        })
        .collect();
    LocalTileBatch {
        id: LocalBatchId::Contour(LocalTileId {
            zoom_bucket: 6,
            lat_bucket: 0,
            lon_bucket: 0,
        }),
        version,
        instances: Arc::new(instances),
        occlusion: occlusion_geometry::build(&paths),
    }
}
fn line(y: f32, z: f32) -> ContourPath {
    ContourPath {
        elevation_m: z,
        points: vec![
            GeoPoint { lon: -1.0, lat: y },
            GeoPoint { lon: 1.0, lat: y },
        ],
    }
}
fn lit(pixels: &[u8], row: usize) -> usize {
    (64..192)
        .filter(|&x| pixels[(row * 256 + x) * 4] > 20)
        .count()
}

#[test]
#[ignore = "requires Metal/GPU; creates only private offscreen textures"]
fn contour_occlusion_hides_far_lines_preserves_crests_and_releases_cache() {
    let mut h = Harness::new([256, 256]);
    let p = params();
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(256.0, 256.0));
    let b = batch(
        vec![
            line(0.0, 1.0),
            line(1.0, 0.0),
            line(-0.5, 0.0),
            line(1.0, 0.8),
        ],
        1,
    );
    h.render(std::slice::from_ref(&b), &p, false, rect, &[]);
    let off = h.pixels();
    assert!(
        lit(&off, 87) > 100 && lit(&off, 147) > 100 && lit(&off, 47) > 100 && lit(&off, 23) > 100
    );
    h.render(std::slice::from_ref(&b), &p, true, rect, &[]);
    let on = h.pixels();
    assert_eq!(lit(&on, 87), 0, "far valley must be hidden");
    assert!(lit(&on, 147) > 100, "near foreground stays visible");
    assert!(lit(&on, 47) > 100, "ridge crest stays visible");
    assert!(lit(&on, 23) > 100, "higher distant terrain stays visible");
    let renders = h.renders();
    h.render(std::slice::from_ref(&b), &p, true, rect, &[]);
    assert_eq!(h.renders(), renders, "stationary depth is reused");
    // Camera change reprojects cached geometry. A new source version must
    // remove the old ridge mask immediately, even at identical camera inputs.
    let mut moved = p;
    moved.yaw_sin = 0.2;
    moved.yaw_cos = (1.0f32 - 0.04).sqrt();
    h.render(std::slice::from_ref(&b), &moved, true, rect, &[]);
    assert_eq!(h.renders(), renders + 1);
    let flat = batch(vec![line(1.0, 0.0)], 2);
    h.render(std::slice::from_ref(&flat), &p, true, rect, &[]);
    assert!(
        lit(&h.pixels(), 87) > 100,
        "old ridge cannot survive replacement"
    );
    h.render(std::slice::from_ref(&b), &p, true, rect, &[]);
    let mut road = flat.clone();
    road.id = LocalBatchId::Road {
        major: true,
        chunk: 0,
    };
    road.occlusion = Default::default();
    h.render(std::slice::from_ref(&b), &p, true, rect, &[road]);
    assert!(
        lit(&h.pixels(), 87) > 100,
        "roads keep independent uniforms"
    );
    h.render(std::slice::from_ref(&b), &p, false, rect, &[]);
    assert!(lit(&h.pixels(), 87) > 100);
    assert!(
        h.renderer
            .callback_resources
            .get::<LocalContourPassResources>()
            .unwrap()
            .occlusion
            .key
            .is_empty()
    );
    // Nonzero canvas origin: texture addressing must use callback coordinates.
    let inset = egui::Rect::from_min_max(egui::pos2(32.0, 16.0), egui::pos2(224.0, 240.0));
    h.render(&[b], &p, true, inset, &[]);
    assert_eq!(lit(&h.pixels(), 87), 0);
    h.render(&[], &p, true, rect, &[]);
    assert!(
        h.renderer
            .callback_resources
            .get::<LocalContourPassResources>()
            .unwrap()
            .tiles
            .is_empty()
    );
}

#[test]
#[ignore = "offscreen GPU visual fixture and frame-time benchmark"]
fn contour_occlusion_visual_and_timing() {
    let mut paths = vec![];
    // Two overlapping synthetic hills, each with exact elliptical isolines.
    for (cx, cy) in [(-0.5, -0.1), (0.4, 0.9)] {
        for level in 1..=80 {
            let z = level as f32 / 80.0;
            let radius = (1.0 - z).sqrt();
            paths.push(ContourPath {
                elevation_m: z,
                points: (0..=1024)
                    .map(|i| {
                        let a = i as f32 / 1024.0 * std::f32::consts::TAU;
                        GeoPoint {
                            lon: cx + radius * a.cos(),
                            lat: cy + radius * a.sin(),
                        }
                    })
                    .collect(),
            });
        }
    }
    let b = batch(paths, 100);
    eprintln!(
        "fixture: {} line segments, {} curtains",
        b.instances.len(),
        b.occlusion.segments.len()
    );
    let mut h = Harness::new([768, 512]);
    let mut p = params();
    p.focus_center_x = 384.0;
    p.focus_center_y = 360.0;
    p.horizontal_scale = 220.0;
    p.ground_pitch_scale = 90.0;
    p.elevation_depth_scale = 190.0;
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(768.0, 512.0));
    for (on, name) in [(false, "before"), (true, "after")] {
        for _ in 0..5 {
            h.render(std::slice::from_ref(&b), &p, on, rect, &[]);
        }
        image::save_buffer(
            format!("/tmp/1kee-occlusion-{name}.png"),
            &h.pixels(),
            768,
            512,
            image::ColorType::Rgba8,
        )
        .unwrap();
        for moving in [false, true] {
            let mut samples = vec![];
            for i in 0..40 {
                if moving {
                    p.focus_lon = i as f32 * 0.0001;
                }
                let start = Instant::now();
                h.render(std::slice::from_ref(&b), &p, on, rect, &[]);
                samples.push(start.elapsed().as_secs_f64() * 1000.0);
            }
            samples.sort_by(f64::total_cmp);
            eprintln!(
                "occlusion={on} moving={moving}: median {:.3} ms p95 {:.3} ms",
                samples[20], samples[38]
            );
        }
        p.focus_lon = 0.0;
    }
}

#[test]
#[ignore = "requires Metal/GPU; exercises camera extremes"]
fn contour_occlusion_camera_extremes_preserve_depth_order() {
    let mut h = Harness::new([256, 256]);
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(256.0, 256.0));
    let mut version = 1000;
    for pitch in [0.0f32, 0.8, 1.55] {
        for yaw in [0.0f32, 1.3, 3.0] {
            for spread in [0.15f32, 1.0, 100.0] {
                version += 1;
                let mut p = params();
                p.focus_lon = 179.9;
                p.focus_lat = 40.0;
                p.x_factor = 100.0;
                p.y_factor = 100.0;
                p.z_factor = 0.001;
                p.pitch_cos = pitch.cos();
                p.pitch_sin = pitch.sin();
                p.yaw_cos = yaw.cos();
                p.yaw_sin = yaw.sin();
                p.ground_pitch_scale = 256.0 * 0.55;
                p.ground_depth_scale = 256.0 * 0.01;
                p.elevation_pitch_scale = 256.0 * 0.55 * spread;
                p.elevation_depth_scale = 256.0 * 0.24 * spread;
                let a = p.pitch_cos * p.ground_pitch_scale - p.pitch_sin * p.ground_depth_scale;
                let z = (p.pitch_sin * p.elevation_pitch_scale
                    + p.pitch_cos * p.elevation_depth_scale)
                    * p.z_factor;
                let make = |ground: f32, height: f32| ContourPath {
                    elevation_m: height / z,
                    points: [-1.0f32, 1.0]
                        .map(|x| GeoPoint {
                            lon: p.focus_lon + (x * p.yaw_cos + ground * p.yaw_sin) / p.x_factor,
                            lat: p.focus_lat + (-x * p.yaw_sin + ground * p.yaw_cos) / p.y_factor,
                        })
                        .to_vec(),
                };
                let b = batch(
                    vec![make(0.0, 80.0), make(40.0 / a, 0.0), make(-20.0 / a, 0.0)],
                    version,
                );
                h.render(&[b], &p, true, rect, &[]);
                let pixels = h.pixels();
                assert_eq!(
                    lit(&pixels, 87),
                    0,
                    "pitch={pitch} yaw={yaw} spread={spread}"
                );
                assert!(
                    lit(&pixels, 147) > 100,
                    "foreground pitch={pitch} yaw={yaw} spread={spread}"
                );
                assert!(
                    lit(&pixels, 47) > 100,
                    "crest pitch={pitch} yaw={yaw} spread={spread}"
                );
            }
        }
    }
}

#[test]
#[ignore = "read-only real terrain fixture; set ONEKEE_OCCLUSION_DB"]
fn contour_occlusion_real_terrain() {
    let Ok(path) = std::env::var("ONEKEE_OCCLUSION_DB") else {
        return;
    };
    let conn =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    // A bounded cached European mountain region; no source acquisition/writes.
    let mut stmt=conn.prepare("SELECT lat_bucket,lon_bucket,elevation_m,geom FROM contour_tiles WHERE zoom_bucket=6 AND lat_bucket BETWEEN 561 AND 563 AND lon_bucket BETWEEN 116 AND 118 ORDER BY lat_bucket,lon_bucket,fid").unwrap();
    let mut rows = stmt.query([]).unwrap();
    let mut by_tile = std::collections::BTreeMap::<(i32, i32), Vec<ContourPath>>::new();
    while let Some(row) = rows.next().unwrap() {
        let lat = row.get(0).unwrap();
        let lon = row.get(1).unwrap();
        let elevation_m = row.get(2).unwrap();
        let bytes: Vec<u8> = row.get(3).unwrap();
        for points in tile_archive::gpkg::parse_gpkg_lines(&bytes, |lon, lat| GeoPoint { lon, lat })
        {
            by_tile.entry((lat, lon)).or_default().push(ContourPath {
                points,
                elevation_m,
            });
        }
    }
    assert!(!by_tile.is_empty(), "fixture region not cached");
    let mut bounds = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    let mut batches = vec![];
    let start = Instant::now();
    for ((lat, lon), paths) in by_tile {
        use crate::panels::world_map::{
            contour_asset::LocalTileGeometry, local_terrain_scene::composition,
        };
        let id = LocalTileId {
            zoom_bucket: 6,
            lat_bucket: lat,
            lon_bucket: lon,
        };
        let tile = LocalTileGeometry {
            id,
            contours: Arc::new(paths),
            bounds: None,
        };
        let paths = composition::clip_contours(
            &tile,
            &[composition::core(crate::model::ActiveBody::Earth, id)],
        );
        for c in paths.iter() {
            for point in &c.points {
                for (i, v) in [point.lon, point.lat, c.elevation_m]
                    .into_iter()
                    .enumerate()
                {
                    bounds[i] = bounds[i].min(v);
                    max[i] = max[i].max(v);
                }
            }
        }
        let mut b = batch((*paths).clone(), 2000);
        b.id = LocalBatchId::Contour(LocalTileId {
            zoom_bucket: 6,
            lat_bucket: lat,
            lon_bucket: lon,
        });
        batches.push(b);
    }
    eprintln!(
        "real fixture: {} tiles, {} lines, {} curtains; worker build {:.1} ms",
        batches.len(),
        batches.iter().map(|b| b.instances.len()).sum::<usize>(),
        batches
            .iter()
            .map(|b| b.occlusion.segments.len())
            .sum::<usize>(),
        start.elapsed().as_secs_f64() * 1000.0
    );
    let mut p = params();
    p.focus_lon = (bounds[0] + max[0]) * 0.5;
    p.focus_lat = (bounds[1] + max[1]) * 0.5;
    p.x_factor = 2.0 / (max[0] - bounds[0]);
    p.y_factor = 2.0 / (max[1] - bounds[1]);
    p.z_factor = 1.0 / (max[2] - bounds[2]);
    p.horizontal_scale = 340.0;
    p.ground_pitch_scale = 110.0;
    p.elevation_depth_scale = 180.0;
    p.focus_center_x = 384.0;
    p.focus_center_y = 360.0 + bounds[2] * p.z_factor * p.elevation_depth_scale;
    let mut h = Harness::new([768, 512]);
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(768.0, 512.0));
    for (on, name) in [(false, "before"), (true, "after")] {
        for _ in 0..80 {
            h.render(&batches, &p, on, rect, &[]);
        }
        image::save_buffer(
            format!("/tmp/1kee-occlusion-real-{name}.png"),
            &h.pixels(),
            768,
            512,
            image::ColorType::Rgba8,
        )
        .unwrap();
        let mut samples = vec![];
        for i in 0..40 {
            p.focus_lon += (max[0] - bounds[0]) * 0.00001 * (if i % 2 == 0 { 1.0 } else { -1.0 });
            let t = Instant::now();
            h.render(&batches, &p, on, rect, &[]);
            samples.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        samples.sort_by(f64::total_cmp);
        eprintln!(
            "real moving occlusion={on}: median {:.3} ms p95 {:.3} ms",
            samples[20], samples[38]
        );
    }
}
