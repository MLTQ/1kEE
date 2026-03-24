use crate::args::RoadsBboxCommand;
use crate::job::{BuildEvent, BuildJob, JobHandle, spawn_job};
use eframe::egui;
use std::fs;
use std::path::PathBuf;
use std::sync::mpsc::TryRecvError;
use std::time::{Duration, SystemTime};

pub struct BuilderApp {
    form: BuilderForm,
    assets: AssetSelection,
    log_lines: Vec<String>,
    status: String,
    progress: f32,
    progress_detail: String,
    inspector: CacheInspector,
    active_job: Option<JobHandle>,
}

#[derive(Clone)]
struct BuilderForm {
    planet_path: String,
    cache_dir: String,
    min_lat: String,
    max_lat: String,
    min_lon: String,
    max_lon: String,
    margin_deg: String,
}

#[derive(Clone)]
struct AssetSelection {
    roads: bool,
    water: bool,
    buildings: bool,
    boundaries: bool,
}

#[derive(Default)]
struct CacheInspector {
    road_cell_count: usize,
    node_cache_count: usize,
    total_bytes: u64,
    latest_files: Vec<String>,
    last_refresh_label: String,
}

impl BuilderApp {
    pub fn new() -> Self {
        let default_cache_dir = std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join("Derived")
            .join("osm")
            .join("road_cells");
        let mut app = Self {
            form: BuilderForm {
                planet_path: "/Volumes/Hilbert/Data/planet-latest.osm.pbf".to_owned(),
                cache_dir: default_cache_dir.display().to_string(),
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
                boundaries: false,
            },
            log_lines: vec!["Ready.".to_owned()],
            status: "Idle".to_owned(),
            progress: 0.0,
            progress_detail: "No active build".to_owned(),
            inspector: CacheInspector::default(),
            active_job: None,
        };
        app.refresh_inspector();
        app
    }

    fn start_build(&mut self) {
        if self.active_job.is_some() {
            return;
        }
        if !self.assets.roads {
            self.push_log(
                "No implemented assets selected. Roads is currently the only export path."
                    .to_owned(),
            );
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
        self.progress_detail = "Starting offline road export…".to_owned();
        self.push_log(format!(
            "Starting roads export for bbox [{}, {}] x [{}, {}]",
            self.form.min_lat, self.form.max_lat, self.form.min_lon, self.form.max_lon
        ));
        self.active_job = Some(spawn_job(BuildJob::Roads(command)));
    }

    fn build_command(&self) -> Result<RoadsBboxCommand, String> {
        let parse_num = |label: &str, value: &str| {
            value
                .parse::<f32>()
                .map_err(|_| format!("Invalid {} value '{}'", label, value))
        };

        let command = RoadsBboxCommand {
            planet_path: PathBuf::from(self.form.planet_path.trim()),
            cache_dir: PathBuf::from(self.form.cache_dir.trim()),
            min_lat: parse_num("min latitude", &self.form.min_lat)?,
            max_lat: parse_num("max latitude", &self.form.max_lat)?,
            min_lon: parse_num("min longitude", &self.form.min_lon)?,
            max_lon: parse_num("max longitude", &self.form.max_lon)?,
            margin_degrees: parse_num("margin degrees", &self.form.margin_deg)?,
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
                if path.extension().and_then(|ext| ext.to_str()) != Some("geojson") {
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
                ui.text_edit_singleline(&mut self.form.planet_path);
                ui.label("Cache Dir");
                ui.text_edit_singleline(&mut self.form.cache_dir);

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

                ui.separator();
                ui.heading("Assets");
                ui.checkbox(&mut self.assets.roads, "Roads");
                ui.add_enabled_ui(false, |ui| {
                    ui.checkbox(&mut self.assets.water, "Water (planned)");
                    ui.checkbox(&mut self.assets.buildings, "Buildings (planned)");
                    ui.checkbox(&mut self.assets.boundaries, "Boundaries (planned)");
                });

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
                        .add_enabled(!building, egui::Button::new("Build Cache"))
                        .clicked()
                    {
                        self.start_build();
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
