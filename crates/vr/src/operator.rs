//! The native map and wrist UI have their own frame cadence and GPU device.
//! Only completed images cross into the headset loop; eye poses never enter the worker.
use anyhow::{Context, Result, anyhow};
use one_thousand_electric_eye_desktop::vr_operator::{MapFrame, Operator, Target, View};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};
pub const MAP_SIZE: [u32; 2] = [1024, 1024];
pub const ROOM_SIZE: [u32; 2] = [832, 384];
pub const WRIST_SIZE: [u32; 2] = [768, 640];

pub struct MapImage {
    pub pixels: Vec<u8>,
    pub frame: MapFrame,
}
#[derive(Default)]
pub struct Updates {
    pub map: Option<MapImage>,
    pub room: Option<Vec<u8>>,
    pub show_room: Option<bool>,
    pub wrist: Option<Vec<u8>>,
    pub state: Option<(u64, View, u64, bool)>,
    pub error: Option<String>,
}
#[derive(Default)]
struct Requests {
    events: Vec<egui::Event>,
    nav: Option<(u64, View)>,
    picks: Vec<Target>,
    visible: bool,
    stop: bool,
}
pub struct Link {
    input: Arc<Mutex<Requests>>,
    output: Arc<Mutex<Updates>>,
    pub revision: u64,
    thread: Option<thread::JoinHandle<()>>,
}
impl Link {
    pub fn start(
        gpu_name: String,
        lat: f32,
        lon: f32,
        root: Option<PathBuf>,
        events: Option<PathBuf>,
        import: Option<PathBuf>,
    ) -> Result<Self> {
        let input = Arc::new(Mutex::new(Requests {
            visible: true,
            ..Default::default()
        }));
        let output = Arc::new(Mutex::new(Updates::default()));
        let (incoming, outgoing) = (input.clone(), output.clone());
        let thread = thread::Builder::new()
            .name("1kee-vr-operator".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let mut worker =
                        Worker::new(&gpu_name, lat, lon, root.as_deref(), events.as_deref())?;
                    if let Some(path) = import {
                        worker.operator.import(path);
                    }
                    worker.run(&incoming, &outgoing)
                }));
                let error = match result {
                    Ok(Err(e)) => Some(format!("{e:#}")),
                    Err(_) => Some("native map renderer stopped unexpectedly".into()),
                    _ => None,
                };
                if let Some(msg) = error {
                    eprintln!("VR operator: {msg}");
                    outgoing.lock().unwrap().error = Some(msg);
                }
            })?;
        Ok(Self {
            input,
            output,
            revision: 0,
            thread: Some(thread),
        })
    }
    pub fn take(&self) -> Updates {
        self.output
            .try_lock()
            .map(|mut pending| std::mem::take(&mut *pending))
            .unwrap_or_default()
    }
    pub fn navigate(&mut self, v: View) {
        self.revision += 1;
        self.input.lock().unwrap().nav = Some((self.revision, v));
    }
    pub fn pick(&self, t: Target) {
        self.input.lock().unwrap().picks.push(t);
    }
    pub fn wrist(&self, events: Vec<egui::Event>, visible: bool) {
        let mut q = self.input.lock().unwrap();
        q.visible = visible;
        for e in events {
            if matches!(e, egui::Event::PointerMoved(_))
                && matches!(q.events.last(), Some(egui::Event::PointerMoved(_)))
            {
                q.events.pop();
            }
            q.events.push(e);
        }
    }
}
impl Drop for Link {
    fn drop(&mut self) {
        self.input.lock().unwrap().stop = true;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct Canvas {
    target: wgpu::Texture,
    view: wgpu::TextureView,
    read: wgpu::Buffer,
    size: [u32; 2],
}
impl Canvas {
    fn new(d: &wgpu::Device, size: [u32; 2]) -> Self {
        let target = d.create_texture(&wgpu::TextureDescriptor {
            label: Some("operator canvas"),
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            dimension: wgpu::TextureDimension::D2,
            mip_level_count: 1,
            sample_count: 1,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let read = d.create_buffer(&wgpu::BufferDescriptor {
            label: Some("operator readback"),
            size: u64::from(size[0]) * u64::from(size[1]) * 4,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        Self {
            view: target.create_view(&Default::default()),
            target,
            read,
            size,
        }
    }
    fn pixels(
        &self,
        ctx: &egui::Context,
        output: egui::FullOutput,
        r: &mut egui_wgpu::Renderer,
        d: &wgpu::Device,
        q: &wgpu::Queue,
    ) -> Result<Vec<u8>> {
        for (id, delta) in &output.textures_delta.set {
            r.update_texture(d, q, *id, delta);
        }
        let jobs = ctx.tessellate(output.shapes, output.pixels_per_point);
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: self.size,
            pixels_per_point: output.pixels_per_point,
        };
        let mut encoder = d.create_command_encoder(&Default::default());
        let mut commands = r.update_buffers(d, q, &mut encoder, &jobs, &screen);
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("native operator map or wrist"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                timestamp_writes: None,
            });
            r.render(&mut pass.forget_lifetime(), &jobs, &screen);
        }
        encoder.copy_texture_to_buffer(
            self.target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &self.read,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.size[0] * 4),
                    rows_per_image: None,
                },
            },
            self.target.size(),
        );
        commands.push(encoder.finish());
        q.submit(commands);
        for id in &output.textures_delta.free {
            r.free_texture(id);
        }
        let (tx, rx) = mpsc::sync_channel(1);
        self.read
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |r| {
                let _ = tx.send(r);
            });
        d.poll(wgpu::Maintain::Wait);
        rx.recv()??;
        let pixels = self.read.slice(..).get_mapped_range().to_vec();
        self.read.unmap();
        Ok(pixels)
    }
}
struct Worker {
    ctx: egui::Context,
    operator: Operator,
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: egui_wgpu::Renderer,
    map: Canvas,
    room: Canvas,
    wrist: Canvas,
    start: Instant,
}
fn raw(
    size: [u32; 2],
    id: egui::ViewportId,
    events: Vec<egui::Event>,
    time: f64,
) -> egui::RawInput {
    let rect =
        egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(size[0] as f32, size[1] as f32));
    let mut raw = egui::RawInput {
        viewport_id: id,
        screen_rect: Some(rect),
        time: Some(time),
        max_texture_side: Some(4096),
        events,
        ..Default::default()
    };
    raw.viewports.entry(id).or_default().native_pixels_per_point = Some(1.);
    raw
}
impl Worker {
    fn new(
        name: &str,
        lat: f32,
        lon: f32,
        root: Option<&std::path::Path>,
        events: Option<&std::path::Path>,
    ) -> Result<Self> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..Default::default()
        });
        let adapter = instance
            .enumerate_adapters(wgpu::Backends::VULKAN)
            .into_iter()
            .find(|a| a.get_info().name.contains(name))
            .context("operator Vulkan adapter")?;
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("Independent native operator queue"),
                required_limits: adapter.limits(),
                ..Default::default()
            },
            None,
        ))?;
        let mut renderer =
            egui_wgpu::Renderer::new(&device, wgpu::TextureFormat::Rgba8Unorm, None, 1, false);
        let ctx = egui::Context::default();
        let operator = Operator::new(
            &ctx,
            &device,
            wgpu::TextureFormat::Rgba8Unorm,
            &mut renderer,
            [lat, lon],
            root,
            events,
        )
        .map_err(|e| anyhow!(e))?;
        let room = Canvas::new(&device, ROOM_SIZE);
        let map = Canvas::new(&device, MAP_SIZE);
        let wrist = Canvas::new(&device, WRIST_SIZE);
        Ok(Self {
            ctx,
            operator,
            device,
            queue,
            renderer,
            room,
            map,
            wrist,
            start: Instant::now(),
        })
    }
    fn run(&mut self, input: &Mutex<Requests>, output: &Mutex<Updates>) -> Result<()> {
        let mut revision = 0;
        let mut next_map = Instant::now();
        let mut prev_wrist = Vec::new();
        let mut prev_room = Vec::new();
        loop {
            let started = Instant::now();
            let (events, nav, picks, visible) = {
                let mut req = input.lock().unwrap();
                if req.stop {
                    return Ok(());
                }
                (
                    std::mem::take(&mut req.events),
                    req.nav.take(),
                    std::mem::take(&mut req.picks),
                    req.visible,
                )
            };
            if let Some((seq, view)) = nav {
                revision = seq;
                self.operator.navigate(view);
            }
            for pick in picks {
                self.operator.pick(pick);
            }
            // Both viewports share texture IDs (including live webcams) and have separate UI memory.
            let wrist_id = egui::ViewportId::from_hash_of("operator-wrist");
            let out = self.ctx.run(
                raw(
                    WRIST_SIZE,
                    wrist_id,
                    events,
                    self.start.elapsed().as_secs_f64(),
                ),
                |ctx| {
                    self.operator.tick(ctx, visible);
                },
            );
            if visible {
                let pixels = self.wrist.pixels(
                    &self.ctx,
                    out,
                    &mut self.renderer,
                    &self.device,
                    &self.queue,
                )?;
                if pixels != prev_wrist {
                    prev_wrist.clone_from(&pixels);
                    output.lock().unwrap().wrist = Some(pixels);
                }
            } else {
                for (id, delta) in &out.textures_delta.set {
                    self.renderer
                        .update_texture(&self.device, &self.queue, *id, delta);
                }
                for id in &out.textures_delta.free {
                    self.renderer.free_texture(id);
                }
            }
            output.lock().unwrap().state = Some((
                revision,
                self.operator.view(),
                self.operator.recenter,
                self.operator.quit,
            ));
            let show_room = self.operator.room_visible();
            output.lock().unwrap().show_room = Some(show_room);
            if show_room {
                let id = egui::ViewportId::from_hash_of("room-brief");
                let out = self.ctx.run(
                    raw(ROOM_SIZE, id, vec![], self.start.elapsed().as_secs_f64()),
                    |c| self.operator.paint_room(c),
                );
                let pixels = self.room.pixels(
                    &self.ctx,
                    out,
                    &mut self.renderer,
                    &self.device,
                    &self.queue,
                )?;
                if pixels != prev_room {
                    prev_room.clone_from(&pixels);
                    output.lock().unwrap().room = Some(pixels);
                }
            }
            if started >= next_map {
                let mut frame = None;
                let out = self.ctx.run(
                    raw(
                        MAP_SIZE,
                        egui::ViewportId::ROOT,
                        vec![],
                        self.start.elapsed().as_secs_f64(),
                    ),
                    |ctx| {
                        frame = Some(self.operator.paint_map(ctx));
                    },
                );
                let pixels = self.map.pixels(
                    &self.ctx,
                    out,
                    &mut self.renderer,
                    &self.device,
                    &self.queue,
                )?;
                output.lock().unwrap().map = Some(MapImage {
                    pixels,
                    frame: frame.unwrap(),
                });
                next_map = Instant::now() + Duration::from_millis(100);
            }
            thread::sleep(Duration::from_millis(40).saturating_sub(started.elapsed()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tile_archive::atlas::{Key, Tier, Writer};
    fn snapshot(w: &mut Worker, canvas: &str) -> (MapFrame, Vec<u8>) {
        let mut frame = MapFrame::default();
        let c = match canvas {
            "map" => &w.map,
            "room" => &w.room,
            _ => &w.wrist,
        };
        let id = if canvas == "map" {
            egui::ViewportId::ROOT
        } else {
            egui::ViewportId::from_hash_of(canvas)
        };
        let out = w.ctx.run(
            raw(c.size, id, vec![], w.start.elapsed().as_secs_f64()),
            |ctx| match canvas {
                "map" => frame = w.operator.paint_map(ctx),
                "room" => w.operator.paint_room(ctx),
                _ => w.operator.wrist(ctx),
            },
        );
        let pixels = c
            .pixels(&w.ctx, out, &mut w.renderer, &w.device, &w.queue)
            .unwrap();
        if let Some(dir) = std::env::var_os("ONEKEE_VR_CAPTURE_DIR") {
            image::save_buffer(
                std::path::PathBuf::from(dir).join(format!("{canvas}.png")),
                &pixels,
                c.size[0],
                c.size[1],
                image::ColorType::Rgba8,
            )
            .unwrap();
        }
        (frame, pixels)
    }
    fn wrist_click(w: &mut Worker, label: &str) {
        fn find(s: &egui::Shape, label: &str) -> Option<egui::Pos2> {
            match s {
                egui::Shape::Text(t) if t.galley.text() == label => {
                    Some(t.pos + t.galley.size() * 0.5)
                }
                egui::Shape::Vec(v) => v.iter().find_map(|s| find(s, label)),
                _ => None,
            }
        }
        let mut draw = |events| {
            let out = w.ctx.run(
                raw(
                    WRIST_SIZE,
                    egui::ViewportId::from_hash_of("operator-wrist"),
                    events,
                    w.start.elapsed().as_secs_f64(),
                ),
                |ctx| w.operator.wrist(ctx),
            );
            let point = out.shapes.iter().find_map(|s| find(&s.shape, label));
            w.wrist
                .pixels(&w.ctx, out, &mut w.renderer, &w.device, &w.queue)
                .unwrap();
            point
        };
        let start = Instant::now();
        let p = loop {
            if let Some(p) = draw(vec![]) {
                break p;
            }
            assert!(
                start.elapsed() < Duration::from_secs(2),
                "missing wrist control {label}"
            );
            thread::sleep(Duration::from_millis(20));
        };
        draw(vec![egui::Event::PointerMoved(p)]);
        for pressed in [true, false] {
            draw(vec![egui::Event::PointerButton {
                pos: p,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            }]);
        }
    }
    #[test]
    #[ignore = "requires a Vulkan GPU; set ONEKEE_GPU to its full name"]
    fn native_operator_loads_geographic_tiles_and_selects_event() {
        let dir = tempfile::tempdir().unwrap();
        let terrain = dir.path().join("Derived/terrain");
        std::fs::create_dir_all(&terrain).unwrap();
        std::fs::create_dir_all(dir.path().join("Data")).unwrap();
        let path = terrain.join(tile_archive::atlas::FILE_NAME);
        let writer = Writer::open(&path).unwrap();
        let side = Tier::Terrain.side();
        let values: Vec<_> = (0..side * side)
            .map(|i| 200. + (i % side) as f32 * 6.)
            .collect();
        for x in 0..2 {
            writer
                .put(
                    Key::at(Tier::Terrain, 0.5 + x as f64, 0.5),
                    "VR fixture",
                    &values,
                )
                .unwrap();
        }
        drop(writer);
        let events = dir.path().join("events.sqlite");
        let db = rusqlite::Connection::open(&events).unwrap();
        db.execute_batch("CREATE TABLE stored_events(factal_id TEXT,title TEXT,summary TEXT,severity_value INTEGER,lat REAL,lon REAL,location_name TEXT,occurred_unix INTEGER);
            INSERT INTO stored_events VALUES('one','Mountain survey','Geographic marker registration',3,0.5,0.5,'West',100),('two','East survey','Second terrain tile',2,0.5,1.5,'East',99);").unwrap();
        drop(db);
        let gpu = std::env::var("ONEKEE_GPU").expect("set ONEKEE_GPU");
        let mut w = Worker::new(&gpu, 0.5, 0.5, Some(dir.path()), Some(&events)).unwrap();
        let (_, wrist) = snapshot(&mut w, "wrist");
        assert!(
            wrist
                .as_chunks::<4>()
                .0
                .iter()
                .any(|p| p[0] > 150 && p[1] > 150)
        );
        wrist_click(&mut w, "Local terrain");
        assert!(w.operator.view().local);
        wrist_click(&mut w, "Globe");
        assert!(!w.operator.view().local);
        wrist_click(&mut w, "Satellite");
        wrist_click(&mut w, "Events");
        wrist_click(&mut w, "Follow Events");
        assert!(w.operator.room_visible());
        wrist_click(&mut w, "Follow Events");
        assert!(!w.operator.room_visible());
        let (g, _) = snapshot(&mut w, "map");
        let event = g
            .hits
            .iter()
            .find(|h| h.target == Target::Event("hist-one".into()))
            .expect("cached event marker")
            .target
            .clone();
        w.operator.pick(event);
        assert!(w.operator.room_visible());
        snapshot(&mut w, "room");
        // A genuine georeferenced GeoTIFF follows the desktop import/warp path.
        let tif = dir.path().join("pass.tif");
        let status = std::process::Command::new("gdal_create")
            .args([
                "-q",
                "-of",
                "GTiff",
                "-outsize",
                "32",
                "32",
                "-bands",
                "3",
                "-burn",
                "245",
                "-burn",
                "12",
                "-burn",
                "225",
                "-a_srs",
                "EPSG:4326",
                "-a_ullr",
                "0",
                "1",
                "1",
                "0",
            ])
            .arg(&tif)
            .status()
            .unwrap();
        assert!(status.success());
        wrist_click(&mut w, "Satellite");
        wrist_click(&mut w, "Browse files on wrist");
        wrist_click(&mut w, "pass.tif");
        for lon in [0.5, 1.5] {
            w.operator.navigate(View {
                lat: 0.5,
                lon,
                scale: 1.,
                local: true,
                local_zoom: 24.,
            });
            let start = Instant::now();
            loop {
                let (f, pixels) = snapshot(&mut w, "map");
                if f.local
                    && f.bounds[0] < lon
                    && f.bounds[2] > lon
                    && f.heights.iter().any(|z| *z > 0.0001)
                    && (lon > 1.
                        || pixels
                            .as_chunks::<4>()
                            .0
                            .iter()
                            .any(|p| p[0] > 160 && p[1] < 80 && p[2] > 155))
                {
                    break;
                }
                assert!(
                    start.elapsed() < Duration::from_secs(45),
                    "no measured relief at {lon}, {:?}",
                    f.bounds
                );
                thread::sleep(Duration::from_millis(100));
            }
        }
        if let Some(root) = std::env::var_os("ONEKEE_VR_ASSET_ROOT") {
            let root = std::path::PathBuf::from(root);
            w.operator.use_asset_root(&root);
            w.operator.navigate(View {
                lat: 46.8523,
                lon: -121.7603,
                scale: 1.,
                local: true,
                local_zoom: 16.,
            });
            let start = Instant::now();
            loop {
                let (f, pixels) = snapshot(&mut w, "map");
                let colors: std::collections::HashSet<_> =
                    pixels.as_chunks::<4>().0.iter().step_by(257).collect();
                if colors.len() > 40 && f.heights.iter().any(|v| v.abs() > 0.01) {
                    eprintln!(
                        "Measured local relief at Mount Rainier loaded after {:.2}s; WGS84 bounds {:?}",
                        start.elapsed().as_secs_f64(),
                        f.bounds
                    );
                    snapshot(&mut w, "wrist");
                    break;
                }
                assert!(
                    start.elapsed() < Duration::from_secs(75),
                    "real terrain did not load: {:?}",
                    w.ctx
                        .data(|d| d.get_temp::<String>(egui::Id::new("vr-terrain-status")))
                );
                thread::sleep(Duration::from_millis(250));
            }
        }
    }
}
