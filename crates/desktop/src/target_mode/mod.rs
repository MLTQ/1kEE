//! Transient Target session, source jobs and a camera independent of Local.
pub(crate) mod camera;
mod context;
mod terrain;
pub(crate) mod project;
mod local_dem;
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
    pub source: Option<Arc<Header>>,
}
static SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
pub(crate) fn serial() -> u64 {
    SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

pub(crate) struct TargetMode {
    pub radius_km: f64,
    pub selecting: bool,
    pub hover: Option<Region>,
    pub session: Option<Session>,
}
impl Default for TargetMode {
    fn default() -> Self {
        Self {
            radius_km: 1.,
            selecting: false,
            hover: None,
            session: None,
        }
    }
}
impl TargetMode {
    pub fn radius_control(ui: &mut egui::Ui, radius: &mut f64) -> egui::Response {
        ui.add(egui::Slider::new(radius, 0.1..=100.)
            .logarithmic(true).step_by(0.1).max_decimals(1).text("km radius"))
            .on_hover_text("Target radius: 100 m to 100 km. Point count and loading remain budgeted independently.")
    }
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
        *self = Self {
            radius_km: self.radius_km,
            ..Self::default()
        };
        ctx.send_viewport_cmd(egui::ViewportCommand::CursorGrab(egui::CursorGrab::None));
        ctx.send_viewport_cmd(egui::ViewportCommand::CursorVisible(true));
        ctx.request_repaint();
    }
}

enum Message {
    Sources(Vec<Source>),
    Header(Header),
    Batch(Arc<Batch>),
    Report(point_cloud::merge::Report),
    Progress(String),
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
    pub merge_surveys: bool,
    pub headers: Vec<Arc<Header>>,
    pub merge_report: Option<point_cloud::merge::Report>,
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
        Self::configured(region, catalog, root, input, 8, None, ctx)
    }
    pub fn resized(
        &self,
        region: Region,
        catalog: PathBuf,
        root: Option<PathBuf>,
        input: ContextInput,
        ctx: &egui::Context,
    ) -> Result<Self, String> {
        let chosen = if self.merge_surveys {None} else {self.sources.get(self.selected_source).cloned().or_else(||self.source.as_ref().map(|h|h.source.clone()))};
        let mut next = Self::configured(
            region,
            catalog,
            root,
            input,
            self.budget_millions,
            chosen,
            ctx,
        )?;
        next.show_points = self.show_points;
        next.show_terrain = self.show_terrain;
        next.show_context = self.show_context;
        next.class_mask = self.class_mask;
        next.color_mode = self.color_mode;
        next.point_size = self.point_size;
        Ok(next)
    }
    fn configured(
        region: Region,
        catalog: PathBuf,
        root: Option<PathBuf>,
        input: ContextInput,
        budget: usize,
        chosen: Option<Source>,
        ctx: &egui::Context,
    ) -> Result<Self, String> {
        if !region.valid() {
            return Err("Target selection is outside supported coordinates".into());
        }
        let active = tempfile::NamedTempFile::new().map_err(|e| e.to_string())?;
        let (tx, rx) = mpsc::channel();
        let worker_ctx = ctx.clone();
        let marker = active.path().to_owned();
        let terrain_catalog = catalog.clone();
        let spawn = std::thread::Builder::new()
            .name("target-dem".into())
            .spawn(move || {
                let result = local_dem::load(&terrain_catalog,region,&marker)
                    .and_then(|local|local.map_or_else(||terrain::load(region,&marker),Ok));
                let _ = tx.send(result);
                worker_ctx.request_repaint();
            })
            .map_err(|e| e.to_string());
        let mut session = Self {
            id: serial(),
            region,
            camera: Camera::for_radius(region.radius_m as f32),
            clouds: Vec::new(),
            points: 0,
            source: None,
            sources: Vec::new(),
            selected_source: 0,
            merge_surveys: chosen.is_none(),
            headers: Vec::new(),
            merge_report: None,
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
            budget_millions: budget,
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
        session.start_points(chosen, ctx);
        Ok(session)
    }
    pub fn compatible_dem(&self) -> bool {
        let known=if self.headers.is_empty() {
            self.source.as_ref().is_some_and(|h|point_cloud::merge::vertical_reference(h).as_deref()==Some("EPSG:5703"))
        } else {self.headers.iter().all(|h|point_cloud::merge::vertical_reference(h).as_deref()==Some("EPSG:5703"))};
        known && self.terrain.as_ref().is_some_and(|t|t.navd88)
    }
    pub fn unverified_heights(&self)->bool {
        if self.headers.is_empty() {self.source.as_ref().is_some_and(|h|point_cloud::merge::vertical_reference(h).is_none())}
        else {self.headers.iter().any(|h|point_cloud::merge::vertical_reference(h).is_none())}
    }
    pub fn start_points(&mut self, chosen: Option<Source>, ctx: &egui::Context) {
        self.point_active = None;
        self.point_rx = None;
        self.summary = None;
        self.merge_surveys = chosen.is_none();
        self.merge_report = None;
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
            let mut current:Option<Arc<Header>>=None;
            let result=point_cloud::merge::load(&catalog,region,chosen,Some(&marker),budget,
                |crs,points|project::xy(crs,points,&marker),|event| {
                    use point_cloud::merge::MergeEvent;
                    let message=match event {
                        MergeEvent::Sources(sources)=>Message::Sources(sources),
                        MergeEvent::Reading {source,refining}=>Message::Progress(format!("{} · {}",
                            if refining {"Adding detail"} else {"Loading coverage"},source.name)),
                        MergeEvent::Data(Event::Header(header))=>{
                            current=Some(Arc::new(header.clone()));Message::Header(header)
                        }
                        MergeEvent::Data(Event::Batch {points,..})=>Message::Batch(Arc::new(Batch {
                            id:serial(),source:current.clone(),points:points.iter().map(|p|vertex(region,p)).collect(),
                        })),
                    };
                    tx.send(message).map_err(|_|"Target closed".to_string())?;
                    worker_ctx.request_repaint();Ok(())
                });
            let done=match result {
                Ok(report)=>{
                    let total=report.total.clone();
                    let _=tx.send(Message::Report(report));Ok(total)
                }
                Err(error)=>Err(error),
            };
            let _=tx.send(Message::Done(done));worker_ctx.request_repaint();
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
    pub fn survey_label(&self) -> &str {
        if let Some(header)=&self.source {return &header.source.name;}
        if self.point_rx.is_some() {
            if let Some(header)=&self.pending_header {return &header.source.name;}
            if let Some(source)=self.sources.get(self.selected_source) {return &source.name;}
            "Finding surveys…"
        } else {"No point cloud loaded"}
    }
    pub fn poll(&mut self, ctx: &egui::Context) {
        for _ in 0..4 {
            let message = self.point_rx.as_ref().map(|r| r.try_recv());
            match message {
                Some(Ok(Message::Sources(sources))) => {
                    self.sources = sources;
                    if !self.replacing && let Some(header) = &self.source
                        && let Some(index) =
                            self.sources.iter().position(|s| s.id == header.source.id)
                    {
                        self.selected_source = index;
                    }
                }
                Some(Ok(Message::Header(h))) => {
                    self.pending_header = Some(h);
                }
                Some(Ok(Message::Progress(status)))=>self.status=status,
                Some(Ok(Message::Report(report)))=>self.merge_report=Some(report),
                Some(Ok(Message::Batch(batch))) => {
                    let header=batch.source.clone().or_else(||self.pending_header.clone().map(Arc::new));
                    if self.replacing {
                        self.headers.clear();
                        self.clouds.clear();
                        self.points = 0;
                        self.source = header.as_deref().cloned();
                        self.replacing = false;
                        if let Some(header) = &self.source
                            && let Some(index) =
                                self.sources.iter().position(|s| s.id == header.source.id)
                        {
                            self.selected_source = index;
                        }
                    }
                    if let Some(header)=header && !self.headers.iter().any(|h|h.source.id==header.source.id) {
                        self.headers.push(header);
                    }
                    if self.show_points && !self.compatible_dem() {
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
                                format!("{} measured points · {} survey(s) read", summary.points,self.headers.len().max(1))
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
    pub(crate) fn live_terrain(
        region: Region,
        active: &std::path::Path,
    ) -> Result<Terrain, String> {
        terrain::load(region, active)
    }
    pub(crate) fn live_local_terrain(catalog:&std::path::Path,region:Region,active:&std::path::Path)->Result<Terrain,String> {
        local_dem::load(catalog,region,active)?.ok_or("No retained DEM".into())
    }
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
                source: None,
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
            merge_surveys: true,
            headers: Vec::new(),
            merge_report: None,
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
    fn finished_empty_point_search_does_not_keep_claiming_to_find_surveys() {
        let s=session();
        let mut empty=s;
        empty.source=None;
        assert_eq!(empty.survey_label(),"No point cloud loaded");
        let (_tx,rx)=mpsc::channel();
        empty.point_rx=Some(rx);
        assert_eq!(empty.survey_label(),"Finding surveys…");
        empty.sources.push(Source {id:"known".into(),name:"Known survey".into(),date:String::new()});
        assert_eq!(empty.survey_label(),"Known survey");
    }
    #[test]
    fn merged_batches_keep_each_source_and_later_unknown_heights_disable_dem_alignment() {
        let ctx=egui::Context::default();
        let mut s=session();
        let mut first=s.source.clone().unwrap();first.source.id="west".into();
        let mut second=first.clone();second.source.id="east".into();second.crs=serde_json::json!({"horizontal":"3857"});
        let (tx,rx)=mpsc::channel();s.point_rx=Some(rx);s.replacing=true;s.show_points=true;
        s.terrain=Some(Arc::new(Terrain {batch:Arc::new(Batch {id:serial(),source:None,points:vec![]}),indices:vec![],
            label:"NAVD88 fixture".into(),navd88:true,width:2,height:2,heights:vec![12.;4],origin:[0.,2.],scale:[1.,-1.]}));
        s.show_terrain=true;
        for (x,h) in [(-500.,first),(500.,second)] {
            tx.send(Message::Header(h.clone())).unwrap();
            tx.send(Message::Batch(Arc::new(Batch {id:serial(),source:Some(Arc::new(h)),points:vec![Vertex {position:[x,0.,12.345],color:[255;4],class:2}]}))).unwrap();
            s.poll(&ctx);
            if x<0. {assert!(s.compatible_dem());assert!(s.show_terrain);assert!(!s.unverified_heights());}
        }
        s.poll(&ctx);
        assert_eq!(s.points,2);assert_eq!(s.headers.len(),2);
        assert_eq!(s.clouds[0].source.as_ref().unwrap().source.id,"west");
        assert_eq!(s.clouds[1].source.as_ref().unwrap().source.id,"east");
        assert!(s.unverified_heights());assert!(!s.compatible_dem());assert!(!s.show_terrain);
        assert!(s.merge_surveys);
    }
    #[test]
    fn incoming_points_preserve_an_explicit_dem_only_view() {
        let ctx = egui::Context::default();
        let mut s = session();
        s.show_points = false;
        s.show_terrain = true;
        s.replacing = true;
        let (tx, rx) = mpsc::channel();
        s.point_rx = Some(rx);
        let mut header = s.source.clone().unwrap();
        header.crs = serde_json::json!({"horizontal":"3857"});
        tx.send(Message::Header(header)).unwrap();
        tx.send(Message::Batch(Arc::new(Batch {
            source: None,
            id: serial(),
            points: vec![Vertex {
                position: [0., 0., 12.],
                color: [255; 4],
                class: 2,
            }],
        })))
        .unwrap();
        s.poll(&ctx);
        assert!(!s.show_points);
        assert!(s.show_terrain);
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
            source: None,
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
            radius_km: 100.,
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
        assert_eq!(mode.radius_km, 100.);
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
