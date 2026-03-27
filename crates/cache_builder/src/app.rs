use crate::args::{BboxCommand, ContoursBboxCommand};
use crate::job::{BuildEvent, BuildJob, JobHandle, spawn_job};
use eframe::egui;
use eframe::egui::Color32;
use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::mpsc::TryRecvError;
use std::time::{Duration, SystemTime};

// ── Natural Earth 110m country outlines (embedded at compile time) ────────────
const NE_COUNTRIES_JSON: &str = include_str!("../assets/ne_110m_countries.json");
static COASTLINES: OnceLock<Vec<Vec<[f32; 2]>>> = OnceLock::new();

/// Returns all outer/inner rings from the Natural Earth 110m FeatureCollection.
/// Parsed once and cached for the lifetime of the process.
fn coastline_rings() -> &'static Vec<Vec<[f32; 2]>> {
    COASTLINES.get_or_init(|| parse_ne_rings(NE_COUNTRIES_JSON))
}

fn parse_ne_rings(json: &str) -> Vec<Vec<[f32; 2]>> {
    let mut rings: Vec<Vec<[f32; 2]>> = Vec::new();
    let Ok(val) = serde_json::from_str::<serde_json::Value>(json) else {
        return rings;
    };
    let Some(features) = val["features"].as_array() else {
        return rings;
    };
    for feat in features {
        let geom = &feat["geometry"];
        match geom["type"].as_str().unwrap_or("") {
            "Polygon" => {
                if let Some(coords) = geom["coordinates"].as_array() {
                    for ring in coords {
                        rings.push(extract_ring(ring));
                    }
                }
            }
            "MultiPolygon" => {
                if let Some(polys) = geom["coordinates"].as_array() {
                    for poly in polys {
                        if let Some(poly_rings) = poly.as_array() {
                            for ring in poly_rings {
                                rings.push(extract_ring(ring));
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    rings
}

fn extract_ring(ring: &serde_json::Value) -> Vec<[f32; 2]> {
    ring.as_array()
        .map(|pts| {
            pts.iter()
                .filter_map(|pt| {
                    let arr = pt.as_array()?;
                    let lon = arr.first()?.as_f64()? as f32;
                    let lat = arr.get(1)?.as_f64()? as f32;
                    Some([lon, lat])
                })
                .collect()
        })
        .unwrap_or_default()
}

pub struct BuilderApp {
    form: BuilderForm,
    assets: AssetSelection,
    log_lines: Vec<String>,
    status: String,
    progress: f32,
    progress_detail: String,
    inspector: CacheInspector,
    active_job: Option<JobHandle>,
    drag_start: Option<egui::Pos2>,
}

#[derive(Clone)]
struct BuilderForm {
    planet_path: String,
    cache_dir: String,
    srtm_root: String,
    min_lat: String,
    max_lat: String,
    min_lon: String,
    max_lon: String,
    margin_deg: String,
    // Contour-specific fields
    contour_db: String,   // path to srtm_focus_cache.sqlite
    gdal_bin_dir: String, // empty = use $PATH
    use_native_engine: bool,
}

#[derive(Clone)]
struct AssetSelection {
    roads: bool,
    water: bool,
    buildings: bool,
    trees: bool,
    admin: bool,
}

#[derive(Default)]
struct CacheInspector {
    road_cell_count: usize,
    node_cache_count: usize,
    total_bytes: u64,
    latest_files: Vec<String>,
    last_refresh_label: String,
}

fn lon_to_x(rect: egui::Rect, lon: f32) -> f32 {
    rect.left() + (lon + 180.0) / 360.0 * rect.width()
}
fn lat_to_y(rect: egui::Rect, lat: f32) -> f32 {
    rect.top() + (90.0 - lat) / 180.0 * rect.height()
}
fn x_to_lon(rect: egui::Rect, x: f32) -> f32 {
    ((x - rect.left()) / rect.width() * 360.0 - 180.0).clamp(-180.0, 180.0)
}
fn y_to_lat(rect: egui::Rect, y: f32) -> f32 {
    (90.0 - (y - rect.top()) / rect.height() * 180.0).clamp(-90.0, 90.0)
}

impl BuilderApp {
    pub fn new() -> Self {
        let default_cache_dir = std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join("Derived")
            .join("osm");
        let mut app = Self {
            form: BuilderForm {
                planet_path: "/Volumes/Hilbert/Data/planet-latest.osm.pbf".to_owned(),
                cache_dir: default_cache_dir.display().to_string(),
                srtm_root: String::new(),
                contour_db: std::env::current_dir()
                    .unwrap_or_else(|_| PathBuf::from("."))
                    .join("Derived")
                    .join("terrain")
                    .join("srtm_focus_cache.sqlite")
                    .display()
                    .to_string(),
                gdal_bin_dir: String::new(),
                use_native_engine: true,
                min_lat: "37.60".to_owned(),
                max_lat: "37.90".to_owned(),
                min_lon: "-122.60".to_owned(),
                max_lon: "-122.20".to_owned(),
                margin_deg: "0.08".to_owned(),
            },
            assets: AssetSelection {
                roads: true,
                water: false,
                buildings: false,
                trees: false,
                admin: false,
            },
            log_lines: vec!["Ready.".to_owned()],
            status: "Idle".to_owned(),
            progress: 0.0,
            progress_detail: "No active build".to_owned(),
            inspector: CacheInspector::default(),
            active_job: None,
            drag_start: None,
        };
        app.refresh_inspector();
        app
    }

    fn start_build(&mut self) {
        if self.active_job.is_some() {
            return;
        }
        let command = match self.build_command() {
            Ok(command) => command,
            Err(error) => {
                self.push_log(error);
                return;
            }
        };

        self.status = "Building".to_owned();
        self.progress = 0.0;
        self.progress_detail = "Starting export…".to_owned();
        self.push_log(format!(
            "Starting export for bbox [{}, {}] x [{}, {}]",
            self.form.min_lat, self.form.max_lat, self.form.min_lon, self.form.max_lon
        ));
        self.active_job = Some(spawn_job(BuildJob::Bbox(command)));
    }

    fn start_contour_build(&mut self) {
        if self.active_job.is_some() {
            return;
        }
        let command = match self.contours_command() {
            Ok(cmd) => cmd,
            Err(e) => { self.push_log(e); return; }
        };
        self.status = "Building Contours".to_owned();
        self.progress = 0.0;
        self.progress_detail = "Starting contour export…".to_owned();
        self.push_log(format!(
            "Starting contour build for bbox [{}, {}] x [{}, {}]",
            self.form.min_lat, self.form.max_lat, self.form.min_lon, self.form.max_lon,
        ));
        self.active_job = Some(spawn_job(BuildJob::ContoursBbox(command)));
    }

    fn contours_command(&self) -> Result<ContoursBboxCommand, String> {
        let parse_num = |label: &str, value: &str| {
            value.parse::<f32>().map_err(|_| format!("Invalid {} value '{}'", label, value))
        };
        let srtm_root = {
            let t = self.form.srtm_root.trim();
            if t.is_empty() {
                return Err("SRTM Root is required for contour building.".to_owned());
            }
            PathBuf::from(t)
        };
        let cache_db = {
            let t = self.form.contour_db.trim();
            if t.is_empty() {
                return Err("Contour DB path is required.".to_owned());
            }
            PathBuf::from(t)
        };
        Ok(ContoursBboxCommand {
            srtm_root,
            cache_db_path: cache_db,
            tmp_dir: None,
            min_lat: parse_num("min latitude", &self.form.min_lat)?,
            max_lat: parse_num("max latitude", &self.form.max_lat)?,
            min_lon: parse_num("min longitude", &self.form.min_lon)?,
            max_lon: parse_num("max longitude", &self.form.max_lon)?,
            zoom_buckets: (0..=6).collect(),
            gdal_bin_dir: PathBuf::from(self.form.gdal_bin_dir.trim()),
            engine: if self.form.use_native_engine {
                crate::args::ContourEngine::Native
            } else {
                crate::args::ContourEngine::Gdal
            },
        })
    }

    fn build_command(&self) -> Result<BboxCommand, String> {
        let parse_num = |label: &str, value: &str| {
            value
                .parse::<f32>()
                .map_err(|_| format!("Invalid {} value '{}'", label, value))
        };

        let srtm_root = {
            let trimmed = self.form.srtm_root.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(PathBuf::from(trimmed))
            }
        };
        let command = BboxCommand {
            planet_path: PathBuf::from(self.form.planet_path.trim()),
            cache_dir: PathBuf::from(self.form.cache_dir.trim()),
            srtm_root,
            min_lat: parse_num("min latitude", &self.form.min_lat)?,
            max_lat: parse_num("max latitude", &self.form.max_lat)?,
            min_lon: parse_num("min longitude", &self.form.min_lon)?,
            max_lon: parse_num("max longitude", &self.form.max_lon)?,
            margin_degrees: parse_num("margin degrees", &self.form.margin_deg)?,
            build_roads: self.assets.roads,
            build_waterways: self.assets.water,
            build_buildings: self.assets.buildings,
            build_trees: self.assets.trees,
            build_admin: self.assets.admin,
        };
        if command.min_lat >= command.max_lat || command.min_lon >= command.max_lon {
            return Err("Invalid bbox: minimums must be less than maximums.".to_owned());
        }
        Ok(command)
    }

    fn poll_job(&mut self) {
        let Some(job) = self.active_job.as_ref() else {
            return;
        };

        let mut events = Vec::new();
        loop {
            match job.receiver.try_recv() {
                Ok(event) => events.push(event),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    events.push(BuildEvent::Finished(Err(
                        "Background build worker disconnected".to_owned(),
                    )));
                    break;
                }
            }
        }

        for event in events {
            match event {
                BuildEvent::Progress(update) => {
                    self.status = update.stage.clone();
                    self.progress = update.fraction.clamp(0.0, 1.0);
                    self.progress_detail = update.message.clone();
                }
                BuildEvent::Log(line) => {
                    self.push_log(line);
                }
                BuildEvent::Finished(result) => {
                    match result {
                        Ok(summary) => {
                            self.status = "Completed".to_owned();
                            self.progress = 1.0;
                            self.progress_detail = summary.clone();
                            self.push_log(summary);
                        }
                        Err(error) => {
                            self.status = "Failed".to_owned();
                            self.progress_detail = error.clone();
                            self.push_log(format!("Build failed: {error}"));
                        }
                    }
                    self.active_job = None;
                    self.refresh_inspector();
                    break;
                }
            }
        }
    }

    fn refresh_inspector(&mut self) {
        let cache_dir = PathBuf::from(self.form.cache_dir.trim());
        let mut files = Vec::new();
        let mut total_bytes = 0u64;
        let mut node_cache_count = 0usize;
        if let Ok(entries) = fs::read_dir(&cache_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let ext = path.extension().and_then(|e| e.to_str());
                if !matches!(ext, Some("1kc") | Some("geojson")) {
                    continue;
                }
                let metadata = entry.metadata().ok();
                let modified = metadata
                    .as_ref()
                    .and_then(|meta| meta.modified().ok())
                    .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
                    .map(|duration| duration.as_secs())
                    .unwrap_or_default();
                total_bytes += metadata.as_ref().map(|meta| meta.len()).unwrap_or_default();
                files.push((modified, path));
            }
        }
        let state_dir = cache_dir.join(".builder_state");
        if let Ok(entries) = fs::read_dir(state_dir) {
            node_cache_count = entries
                .flatten()
                .filter(|entry| {
                    matches!(
                        entry.path().extension().and_then(|ext| ext.to_str()),
                        Some("jsonl" | "sqlite")
                    )
                })
                .count();
        }
        files.sort_by(|left, right| right.0.cmp(&left.0));

        self.inspector.road_cell_count = files.len();
        self.inspector.node_cache_count = node_cache_count;
        self.inspector.total_bytes = total_bytes;
        self.inspector.latest_files = files
            .into_iter()
            .take(10)
            .map(|(_, path)| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or_default()
                    .to_owned()
            })
            .collect();
        self.inspector.last_refresh_label = format!("{:?}", SystemTime::now());
    }

    fn push_log(&mut self, line: String) {
        self.log_lines.push(line);
        if self.log_lines.len() > 200 {
            let drop_count = self.log_lines.len() - 200;
            self.log_lines.drain(0..drop_count);
        }
    }
}

impl eframe::App for BuilderApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_job();
        if self.active_job.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }

        egui::TopBottomPanel::top("builder_header").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("1kEE Cache Builder");
                ui.separator();
                ui.label("Offline planet.osm cache generation");
                ui.separator();
                ui.label(format!("Status: {}", self.status));
            });
        });

        egui::SidePanel::left("builder_controls")
            .resizable(true)
            .default_width(360.0)
            .show(ctx, |ui| {
                ui.heading("Export");
                ui.label(
                    "Select the source planet file, output cache, bbox, and assets to generate.",
                );
                ui.separator();

                ui.label("Planet PBF");
                ui.horizontal(|ui| {
                    ui.text_edit_singleline(&mut self.form.planet_path);
                    if ui.small_button("…").clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("PBF", &["pbf"])
                            .pick_file()
                        {
                            self.form.planet_path = path.display().to_string();
                        }
                    }
                });
                ui.label("Cache Dir");
                ui.horizontal(|ui| {
                    ui.text_edit_singleline(&mut self.form.cache_dir);
                    if ui.small_button("…").clicked() {
                        if let Some(path) = rfd::FileDialog::new().pick_folder() {
                            self.form.cache_dir = path.display().to_string();
                        }
                    }
                });
                ui.label("SRTM Root (optional — bakes elevation into .1kc files)");
                ui.horizontal(|ui| {
                    ui.text_edit_singleline(&mut self.form.srtm_root);
                    if ui.small_button("…").clicked() {
                        if let Some(path) = rfd::FileDialog::new().pick_folder() {
                            self.form.srtm_root = path.display().to_string();
                        }
                    }
                    if !self.form.srtm_root.is_empty() && ui.small_button("✕").clicked() {
                        self.form.srtm_root.clear();
                    }
                });

                ui.separator();
                ui.label("Bounding Box");
                ui.horizontal(|ui| {
                    ui.label("Min Lat");
                    ui.text_edit_singleline(&mut self.form.min_lat);
                    ui.label("Max Lat");
                    ui.text_edit_singleline(&mut self.form.max_lat);
                });
                ui.horizontal(|ui| {
                    ui.label("Min Lon");
                    ui.text_edit_singleline(&mut self.form.min_lon);
                    ui.label("Max Lon");
                    ui.text_edit_singleline(&mut self.form.max_lon);
                });
                ui.horizontal(|ui| {
                    ui.label("Margin");
                    ui.text_edit_singleline(&mut self.form.margin_deg);
                });

                // ── World bbox map ────────────────────────────────────────────
                ui.separator();
                ui.label("Bounding Box Map");

                let (response, painter) = ui.allocate_painter(
                    egui::Vec2::new(280.0, 140.0),
                    egui::Sense::click_and_drag(),
                );
                let rect = response.rect;

                // Background
                painter.rect_filled(rect, 0.0, Color32::from_rgb(8, 12, 28));

                // Graticule lines
                let graticule_color  = Color32::from_rgb(20, 40, 80);
                let equator_color    = Color32::from_rgb(40, 80, 120);
                let lons = [-180i32, -150, -120, -90, -60, -30, 0, 30, 60, 90, 120, 150, 180];
                let lats = [-90i32, -60, -30, 0, 30, 60, 90];
                for lon in lons {
                    let x = lon_to_x(rect, lon as f32);
                    painter.line_segment(
                        [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                        egui::Stroke::new(1.0, graticule_color),
                    );
                }
                for lat in lats {
                    let y = lat_to_y(rect, lat as f32);
                    let color = if lat == 0 { equator_color } else { graticule_color };
                    painter.line_segment(
                        [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
                        egui::Stroke::new(1.0, color),
                    );
                }

                // Country outlines (Natural Earth 110m)
                let coast_color = Color32::from_rgb(70, 100, 120);
                for ring in coastline_rings() {
                    let pts: Vec<egui::Pos2> = ring
                        .iter()
                        .map(|[lon, lat]| egui::pos2(lon_to_x(rect, *lon), lat_to_y(rect, *lat)))
                        .collect();
                    if pts.len() >= 2 {
                        painter.add(egui::Shape::line(pts, egui::Stroke::new(0.5, coast_color)));
                    }
                }

                // Equator label
                let eq_y = lat_to_y(rect, 0.0);
                painter.text(
                    egui::pos2(rect.left() + 2.0, eq_y - 8.0),
                    egui::Align2::LEFT_TOP,
                    "Equator",
                    egui::FontId::proportional(8.0),
                    equator_color,
                );

                // Bbox rectangle from current form values
                let bbox_color  = Color32::from_rgb(220, 160, 30);
                let bbox_fill   = Color32::from_rgba_unmultiplied(220, 160, 30, 38); // ~15% opacity
                if let (Ok(min_lat), Ok(max_lat), Ok(min_lon), Ok(max_lon)) = (
                    self.form.min_lat.trim().parse::<f32>(),
                    self.form.max_lat.trim().parse::<f32>(),
                    self.form.min_lon.trim().parse::<f32>(),
                    self.form.max_lon.trim().parse::<f32>(),
                ) {
                    let x0 = lon_to_x(rect, min_lon);
                    let x1 = lon_to_x(rect, max_lon);
                    let y0 = lat_to_y(rect, max_lat);
                    let y1 = lat_to_y(rect, min_lat);
                    let bbox_rect = egui::Rect::from_min_max(
                        egui::pos2(x0, y0),
                        egui::pos2(x1, y1),
                    );
                    painter.rect_filled(bbox_rect, 0.0, bbox_fill);
                    painter.rect_stroke(bbox_rect, 0.0, egui::Stroke::new(1.5, bbox_color), egui::StrokeKind::Middle);
                }

                // Drag interaction to set bbox
                if response.drag_started() {
                    self.drag_start = response.hover_pos();
                }
                if response.dragged() {
                    if let (Some(start), Some(current)) = (self.drag_start, response.hover_pos()) {
                        let lon0 = x_to_lon(rect, start.x);
                        let lat0 = y_to_lat(rect, start.y);
                        let lon1 = x_to_lon(rect, current.x);
                        let lat1 = y_to_lat(rect, current.y);
                        let min_lat = lat0.min(lat1);
                        let max_lat = lat0.max(lat1);
                        let min_lon = lon0.min(lon1);
                        let max_lon = lon0.max(lon1);
                        self.form.min_lat = format!("{:.4}", min_lat);
                        self.form.max_lat = format!("{:.4}", max_lat);
                        self.form.min_lon = format!("{:.4}", min_lon);
                        self.form.max_lon = format!("{:.4}", max_lon);
                    }
                }
                if response.drag_stopped() {
                    if let (Some(start), Some(end)) = (self.drag_start, response.hover_pos()) {
                        let lon0 = x_to_lon(rect, start.x);
                        let lat0 = y_to_lat(rect, start.y);
                        let lon1 = x_to_lon(rect, end.x);
                        let lat1 = y_to_lat(rect, end.y);
                        self.form.min_lat = format!("{:.4}", lat0.min(lat1));
                        self.form.max_lat = format!("{:.4}", lat0.max(lat1));
                        self.form.min_lon = format!("{:.4}", lon0.min(lon1));
                        self.form.max_lon = format!("{:.4}", lon0.max(lon1));
                    }
                    self.drag_start = None;
                }

                // ── Assets ────────────────────────────────────────────────────
                ui.separator();
                ui.heading("Assets");
                ui.checkbox(&mut self.assets.roads, "Roads");
                ui.checkbox(&mut self.assets.water, "Waterways");
                ui.checkbox(&mut self.assets.buildings, "Buildings");
                ui.checkbox(&mut self.assets.trees, "Trees / Forest");
                ui.checkbox(&mut self.assets.admin, "Admin Boundaries");

                // ── Terrain / Contours ────────────────────────────────────────
                ui.separator();
                ui.heading("Terrain / Contours");
                ui.label("Contour DB folder (contains srtm_focus_cache.sqlite)");
                ui.horizontal(|ui| {
                    ui.text_edit_singleline(&mut self.form.contour_db);
                    if ui.small_button("…").clicked() {
                        // Pick a folder — avoids the macOS "Replace?" dialog that
                        // save_file() triggers when the DB already exists.
                        if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                            self.form.contour_db = folder
                                .join("srtm_focus_cache.sqlite")
                                .display()
                                .to_string();
                        }
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Engine:");
                    ui.radio_value(&mut self.form.use_native_engine, true, "Native (fast, no GDAL)");
                    ui.radio_value(&mut self.form.use_native_engine, false, "GDAL");
                });
                if !self.form.use_native_engine {
                    ui.label("GDAL bin dir (empty = use $PATH)");
                    ui.horizontal(|ui| {
                        ui.text_edit_singleline(&mut self.form.gdal_bin_dir);
                        if ui.small_button("…").clicked() {
                            if let Some(path) = rfd::FileDialog::new().pick_folder() {
                                self.form.gdal_bin_dir = path.display().to_string();
                            }
                        }
                        if !self.form.gdal_bin_dir.is_empty() && ui.small_button("✕").clicked() {
                            self.form.gdal_bin_dir.clear();
                        }
                    });
                }

                ui.separator();
                ui.add(
                    egui::ProgressBar::new(self.progress)
                        .animate(self.active_job.is_some())
                        .show_percentage()
                        .text(self.progress_detail.clone()),
                );

                ui.horizontal(|ui| {
                    let building = self.active_job.is_some();
                    if ui
                        .add_enabled(!building, egui::Button::new("Build OSM Cache"))
                        .clicked()
                    {
                        self.start_build();
                    }
                    if ui
                        .add_enabled(!building, egui::Button::new("Build Contours"))
                        .clicked()
                    {
                        self.start_contour_build();
                    }
                    if ui.button("Refresh Inspector").clicked() {
                        self.refresh_inspector();
                    }
                });
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.columns(2, |columns| {
                columns[0].heading("Inspector");
                columns[0].label(format!(
                    "Road cell files: {}",
                    self.inspector.road_cell_count
                ));
                columns[0].label(format!("Node caches: {}", self.inspector.node_cache_count));
                columns[0].label(format!(
                    "Approx size: {:.2} MiB",
                    self.inspector.total_bytes as f64 / (1024.0 * 1024.0)
                ));
                columns[0].label(format!(
                    "Last refresh: {}",
                    self.inspector.last_refresh_label
                ));
                columns[0].separator();
                egui::ScrollArea::vertical().show(&mut columns[0], |ui| {
                    for name in &self.inspector.latest_files {
                        ui.monospace(name);
                    }
                });

                columns[1].heading("Build Log");
                egui::ScrollArea::vertical().show(&mut columns[1], |ui| {
                    for line in &self.log_lines {
                        ui.label(line);
                    }
                });
            });
        });
    }
}
