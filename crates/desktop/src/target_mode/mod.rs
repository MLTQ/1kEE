//! Transient Target session, source jobs and a camera independent of Local.
pub(crate) mod camera;
mod context;
mod terrain;
use camera::Camera;
pub(crate) use context::{ContextGeometry, ContextInput};
use lidar_pipeline::{
    ept::point_cloud::{self, Event, Header, Source, Summary},
    region::Region,
};
use std::{
    path::PathBuf,
    sync::{Arc, mpsc},
    time::Instant,
};
pub(crate) use terrain::Terrain;

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct Vertex {
    pub position: [f32; 3],
    pub color: [u8; 4],
    pub class: u32,
}
pub(crate) struct Batch {
    pub id: u64,
    pub points: Vec<Vertex>,
}
static SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
pub(crate) fn serial() -> u64 {
    SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

#[derive(Default)]
pub(crate) struct TargetMode {
    pub selecting: bool,
    pub hover: Option<Region>,
    pub session: Option<Session>,
}
impl TargetMode {
    pub fn engaged(&self) -> bool {
        self.selecting || self.session.is_some()
    }
    pub fn consume_escape(&mut self, ctx: &egui::Context) -> bool {
        let escape = self.engaged()
            && ctx.input_mut(|input| input.consume_key(input.modifiers, egui::Key::Escape));
        if escape {
            self.exit(ctx);
        }
        escape
    }
    pub fn exit(&mut self, ctx: &egui::Context) {
        *self = Self::default();
        ctx.send_viewport_cmd(egui::ViewportCommand::CursorGrab(egui::CursorGrab::None));
        ctx.send_viewport_cmd(egui::ViewportCommand::CursorVisible(true));
        ctx.request_repaint();
    }
}

enum Message {
    Sources(Vec<Source>),
    Header(Header),
    Batch(Arc<Batch>),
    Done(Result<Summary, String>),
}
pub(crate) struct Session {
    pub id: u64,
    pub region: Region,
    pub camera: Camera,
    pub clouds: Vec<Arc<Batch>>,
    pub points: usize,
    pub source: Option<Header>,
    pub sources: Vec<Source>,
    pub selected_source: usize,
    pub summary: Option<Summary>,
    pub status: String,
    pub terrain: Option<Arc<Terrain>>,
    pub terrain_status: String,
    pub context: Option<Arc<ContextGeometry>>,
    pub context_status: String,
    pub show_points: bool,
    pub show_terrain: bool,
    pub show_context: bool,
    pub class_mask: u32,
    pub color_mode: u32,
    pub point_size: f32,
    pub budget_millions: usize,
    pub started: Instant,
    catalog: PathBuf,
    point_rx: Option<mpsc::Receiver<Message>>,
    point_active: Option<tempfile::NamedTempFile>,
    replacing: bool,
    pending_header: Option<Header>,
    terrain_rx: Option<mpsc::Receiver<Result<Terrain, String>>>,
    context_rx: Option<mpsc::Receiver<Result<ContextGeometry, String>>>,
    context_input: Option<ContextInput>,
    _active: tempfile::NamedTempFile,
}
impl Session {
    pub fn new(
        region: Region,
        catalog: PathBuf,
        root: Option<PathBuf>,
        input: ContextInput,
        ctx: &egui::Context,
    ) -> Result<Self, String> {
        if !region.valid() {
            return Err("Target selection is outside supported coordinates".into());
        }
        let active = tempfile::NamedTempFile::new().map_err(|e| e.to_string())?;
        let (tx, rx) = mpsc::channel();
        let worker_ctx = ctx.clone();
        let marker = active.path().to_owned();
        let spawn = std::thread::Builder::new()
            .name("target-dem".into())
            .spawn(move || {
                let result = terrain::load(region, &marker);
                let _ = tx.send(result);
                worker_ctx.request_repaint();
            })
            .map_err(|e| e.to_string());
        let mut session = Self {
            id: serial(),
            region,
            camera: Default::default(),
            clouds: Vec::new(),
            points: 0,
            source: None,
            sources: Vec::new(),
            selected_source: 0,
            summary: None,
            status: "Finding point-cloud surveys…".into(),
            terrain: None,
            terrain_status: spawn
                .err()
                .unwrap_or_else(|| "Loading bare-earth DEM…".into()),
            context: None,
            context_status: "Waiting for terrain to drape map context…".into(),
            show_points: true,
            show_terrain: true,
            show_context: true,
            class_mask: u32::MAX,
            color_mode: 0,
            point_size: 2.,
            budget_millions: 8,
            started: Instant::now(),
            catalog,
            point_rx: None,
            point_active: None,
            replacing: true,
            pending_header: None,
            terrain_rx: Some(rx),
            context_rx: None,
            context_input: Some(input.with_root(root)),
            _active: active,
        };
        session.start_points(None, ctx);
        Ok(session)
    }
    pub fn compatible_dem(&self) -> bool {
        self.source.as_ref().is_some_and(|h| {
            h.crs["vertical"].as_str() == Some("5703") || h.crs["vertical"].as_u64() == Some(5703)
        }) && self.terrain.as_ref().is_some_and(|t| t.navd88)
    }
    pub fn start_points(&mut self, chosen: Option<Source>, ctx: &egui::Context) {
        self.point_active = None;
        self.point_rx = None;
        self.summary = None;
        self.replacing = true;
        self.pending_header = None;
        self.status = "Loading original LiDAR returns…".into();
        let Ok(active) = tempfile::NamedTempFile::new() else {
            self.status = "Cannot create point-load cancellation marker".into();
            return;
        };
        let marker = active.path().to_owned();
        let catalog = self.catalog.clone();
        let region = self.region;
        let budget = self.budget_millions * 1_000_000;
        let worker_ctx = ctx.clone();
        let (tx, rx) = mpsc::sync_channel(2);
        let spawned=std::thread::Builder::new().name("target-points".into()).spawn(move || {
            let run=||->Result<Summary,String> {
                let sources=if let Some(source)=chosen {vec![source]} else {point_cloud::sources(&catalog,region)?};
                tx.send(Message::Sources(sources.clone())).map_err(|_|"Target closed")?;
                let mut last="No public point cloud is indexed here; the DEM and map context can still be inspected".to_string();
                for source in sources {
                    if !marker.is_file() {return Err("Target closed".into());}
                    let mut emitted=0;
                    let result=point_cloud::stream(&catalog,region,&source,Some(&marker),budget,|event| {
                        let message=match event {
                            Event::Header(h)=>Message::Header(h),
                            Event::Batch {points,..}=>{
                                emitted+=points.len();
                                Message::Batch(Arc::new(Batch {id:serial(),
                                    points:points.iter().map(|p|vertex(region,p)).collect()}))
                            },
                        };
                        tx.send(message).map_err(|_|"Target closed".to_string())?;
                        worker_ctx.request_repaint();Ok(())
                    });
                    match result {Ok(summary) if summary.points>0=>return Ok(summary),
                        Ok(_)=>last=format!("No returns in {} within this circle",source.name),
                        Err(e) if emitted>0=>return Err(format!("Partial point cloud retained · {e}")),
                        Err(e)=>last=e,}
                }
                Err(last)
            };
            let result=run();let _=tx.send(Message::Done(result));worker_ctx.request_repaint();
        });
        match spawned {
            Ok(_) => {
                self.point_active = Some(active);
                self.point_rx = Some(rx);
            }
            Err(e) => self.status = e.to_string(),
        }
    }
    pub fn loading(&self) -> bool {
        self.point_rx.is_some() || self.terrain_rx.is_some() || self.context_rx.is_some()
    }
    pub fn poll(&mut self, ctx: &egui::Context) {
        for _ in 0..4 {
            let message = self.point_rx.as_ref().map(|r| r.try_recv());
            match message {
                Some(Ok(Message::Sources(sources))) => {
                    if self.sources.is_empty() {
                        self.sources = sources;
                    }
                }
                Some(Ok(Message::Header(h))) => {
                    self.pending_header = Some(h);
                }
                Some(Ok(Message::Batch(batch))) => {
                    if self.replacing {
                        self.clouds.clear();
                        self.points = 0;
                        self.source = self.pending_header.take();
                        self.replacing = false;
                        if let Some(header) = &self.source
                            && let Some(index) =
                                self.sources.iter().position(|s| s.id == header.source.id)
                        {
                            self.selected_source = index;
                        }
                    }
                    if self.points == 0 && !self.compatible_dem() {
                        self.show_terrain = false;
                    }
                    if !self.camera.positioned
                        && let Some(p) = batch.points.first()
                    {
                        self.camera.reset(p.position[2]);
                    }
                    self.points += batch.points.len();
                    self.clouds.push(batch);
                    self.status = format!("Loading · {} measured points", self.points);
                }
                Some(Ok(Message::Done(result))) => {
                    match result {
                        Ok(summary) => {
                            self.status = if summary.complete {
                                format!("{} measured points · source detail loaded", summary.points)
                            } else {
                                format!("{} measured points · {}", summary.points, summary.limit)
                            };
                            self.summary = Some(summary);
                        }
                        Err(error) => {
                            self.status = error;
                            if self.points == 0 {
                                self.show_terrain = true;
                            }
                        }
                    }
                    self.point_rx = None;
                    self.point_active = None;
                    break;
                }
                Some(Err(mpsc::TryRecvError::Disconnected)) => {
                    self.status = "Point worker stopped; existing points retained".into();
                    self.point_rx = None;
                    self.point_active = None;
                    break;
                }
                _ => break,
            }
        }
        if let Some(result) = crate::lidar_catalog::completed(&mut self.terrain_rx) {
            match result {
                Ok(terrain) => {
                    if !self.camera.positioned {
                        self.camera.reset(terrain.center_height());
                    }
                    self.terrain_status = terrain.label.clone();
                    self.terrain = Some(Arc::new(terrain));
                }
                Err(error) => self.terrain_status = error,
            }
            if let Some(input) = self.context_input.take() {
                let terrain = self.terrain.clone();
                let region = self.region;
                let (tx, rx) = mpsc::channel();
                let worker_ctx = ctx.clone();
                let marker = self._active.path().to_owned();
                if std::thread::Builder::new()
                    .name("target-context".into())
                    .spawn(move || {
                        let _ = tx.send(context::build(region, input, terrain, &marker));
                        worker_ctx.request_repaint();
                    })
                    .is_ok()
                {
                    self.context_rx = Some(rx);
                    self.context_status = "Loading cached map context…".into();
                }
            }
        }
        if let Some(result) = crate::lidar_catalog::completed(&mut self.context_rx) {
            match result {
                Ok(context) => {
                    self.context_status = context.label.clone();
                    self.context = Some(Arc::new(context));
                }
                Err(error) => self.context_status = error,
            }
        }
    }
}

pub(crate) fn vertex(region: Region, p: &point_cloud::Point) -> Vertex {
    let [x, y] = region.offset(p.lon_lat[0], p.lon_lat[1]);
    let mut color = match p.classification {
        2 => [180, 154, 114, 255],
        3..=5 => [76, 181, 133, 255],
        6 => [220, 168, 105, 255],
        9 => [75, 163, 219, 255],
        _ => [111, 198, 213, 255],
    };
    if let Some(rgb) = p.rgb {
        color = [
            (rgb[0] >> 8) as u8,
            (rgb[1] >> 8) as u8,
            (rgb[2] >> 8) as u8,
            255,
        ];
    }
    Vertex {
        position: [x as f32, y as f32, p.source_xyz[2] as f32],
        color,
        class: u32::from(p.classification)
            | (u32::from(p.intensity) << 8)
            | (u32::from(p.return_number.min(15)) << 24)
            | if p.rgb.is_some() { 1 << 31 } else { 0 },
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    pub(crate) fn session() -> Session {
        let active = tempfile::NamedTempFile::new().unwrap();
        let region = Region {
            lon: -77.58,
            lat: 35.26,
            radius_m: 1000.,
        };
        Session {
            id: serial(),
            region,
            camera: Default::default(),
            clouds: vec![Arc::new(Batch {
                id: serial(),
                points: vec![Vertex {
                    position: [0., 0., 12.],
                    color: [100, 200, 200, 255],
                    class: 2,
                }],
            })],
            points: 1,
            source: Some(Header {
                source: Source {
                    id: "fixture".into(),
                    name: "Fixture survey".into(),
                    date: "2026".into(),
                },
                crs: serde_json::json!({"vertical":"5703"}),
                publisher: "https://example.org".into(),
            }),
            sources: Vec::new(),
            selected_source: 0,
            summary: None,
            status: "Measured fixture".into(),
            terrain: None,
            terrain_status: "No fixture DEM".into(),
            context: None,
            context_status: String::new(),
            show_points: true,
            show_terrain: false,
            show_context: false,
            class_mask: u32::MAX,
            color_mode: 0,
            point_size: 2.,
            budget_millions: 8,
            started: Instant::now(),
            catalog: PathBuf::new(),
            point_rx: None,
            point_active: None,
            replacing: false,
            pending_header: None,
            terrain_rx: None,
            context_rx: None,
            context_input: None,
            _active: active,
        }
    }
    #[test]
    fn target_refresh_keeps_completed_points_until_a_replacement_batch_and_exit_cancels_jobs() {
        let ctx = egui::Context::default();
        let mut s = session();
        let before = s.clouds[0].id;
        let (tx, rx) = mpsc::channel();
        s.point_rx = Some(rx);
        s.replacing = true;
        let mut header = s.source.clone().unwrap();
        header.source.id = "replacement".into();
        tx.send(Message::Header(header.clone())).unwrap();
        tx.send(Message::Done(Err("service unavailable".into())))
            .unwrap();
        s.poll(&ctx);
        assert_eq!(s.clouds[0].id, before);
        assert_eq!(s.source.as_ref().unwrap().source.id, "fixture");
        let (tx, rx) = mpsc::channel();
        s.point_rx = Some(rx);
        s.replacing = true;
        tx.send(Message::Header(header)).unwrap();
        tx.send(Message::Batch(Arc::new(Batch {
            id: serial(),
            points: vec![Vertex {
                position: [1., 2., 3.],
                color: [255; 4],
                class: 2,
            }],
        })))
        .unwrap();
        tx.send(Message::Done(Err("partial load failed".into())))
            .unwrap();
        s.poll(&ctx);
        assert_ne!(s.clouds[0].id, before);
        assert_eq!(s.source.as_ref().unwrap().source.id, "replacement");
        assert_eq!(s.points, 1);
        s.point_active = Some(tempfile::NamedTempFile::new().unwrap());
        let marker = s.point_active.as_ref().unwrap().path().to_owned();
        let mut mode = TargetMode {
            selecting: false,
            hover: None,
            session: Some(s),
        };
        let _ = ctx.run(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key: egui::Key::Escape,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: Default::default(),
                }],
                ..Default::default()
            },
            |ctx| {
                assert!(mode.consume_escape(ctx));
                assert!(!mode.engaged());
            },
        );
        assert!(!marker.exists());
    }
    #[test]
    fn target_gpu_offsets_preserve_submillimetre_positions_and_original_heights() {
        let r = Region {
            lon: -77.58,
            lat: 35.26,
            radius_m: 1000.,
        };
        let xy = [999.123456, 12.345678];
        let ll = r.location(xy[0], xy[1]);
        let p = point_cloud::Point {
            source_xyz: [-8_600_000.123456, 4_200_000.654321, 123.456789],
            lon_lat: ll,
            classification: 5,
            intensity: 1234,
            return_number: 2,
            rgb: None,
        };
        let v = vertex(r, &p);
        assert!((f64::from(v.position[0]) - xy[0]).abs() < 0.0001);
        assert!((f64::from(v.position[1]) - xy[1]).abs() < 0.0001);
        assert!((f64::from(v.position[2]) - p.source_xyz[2]).abs() < 0.0001);
        assert_eq!(v.class & 255, 5);
        assert_eq!((v.class >> 8) & 65535, 1234);
    }
}
