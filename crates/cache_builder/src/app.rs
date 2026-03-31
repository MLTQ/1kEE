use crate::args::{BboxCommand, ContoursBboxCommand};
use crate::job::{BuildEvent, BuildJob, JobHandle, spawn_job};
use crate::lunar::{LunarBuildCommand, all_lunar_specs};
use eframe::egui;
use eframe::egui::Color32;
use rusqlite::OpenFlags;
use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::mpsc::TryRecvError;
use std::time::{Duration, SystemTime};

#[derive(Clone, Copy, PartialEq)]
enum ActiveTab {
    OsmContours,
    Lunar,
}

#[derive(Clone)]
struct LunarForm {
    jp2_path:    String,
    lunar_db:    String,
    gdal_bin_dir: String,
    min_lat:     String,
    max_lat:     String,
    min_lon:     String,
    max_lon:     String,
    /// Per-bucket checkbox state (index = zoom_bucket 0..=4)
    zoom_buckets: [bool; 5],
}

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
    /// Tiles already in the cache (from Scan Cache): (zoom_bucket, min_lat, max_lat, min_lon, max_lon)
    cached_tiles: Vec<(i32, f32, f32, f32, f32)>,
    /// Tiles completed in the current build run: (min_lat, max_lat, min_lon, max_lon)
    live_tiles: Vec<(f32, f32, f32, f32)>,
    // ── Lunar tab ─────────────────────────────────────────────────────────────
    active_tab: ActiveTab,
    lunar_form: LunarForm,
    lunar_drag_start: Option<egui::Pos2>,
    /// Lunar tiles already in lunar_focus_cache.sqlite
    lunar_cached_tiles: Vec<(i32, f32, f32, f32, f32)>,
    /// Lunar tiles built in the current session
    lunar_live_tiles: Vec<(f32, f32, f32, f32)>,
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
    cell_file_count: usize,
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
            cached_tiles: Vec::new(),
            live_tiles: Vec::new(),
            active_tab: ActiveTab::OsmContours,
            lunar_form: LunarForm {
                jp2_path: String::new(),
                lunar_db: std::env::current_dir()
                    .unwrap_or_else(|_| PathBuf::from("."))
                    .join("Derived")
                    .join("terrain")
                    .join("lunar_focus_cache.sqlite")
                    .display()
                    .to_string(),
                gdal_bin_dir: String::new(),
                min_lat: "-60.0".to_owned(),
                max_lat:  "60.0".to_owned(),
                min_lon: "-180.0".to_owned(),
                max_lon:  "180.0".to_owned(),
                zoom_buckets: [true, true, false, false, false],
            },
            lunar_drag_start: None,
            lunar_cached_tiles: Vec::new(),
            lunar_live_tiles: Vec::new(),
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

    fn scan_cache_tiles(&mut self) {
        let db_path = PathBuf::from(self.form.contour_db.trim());
        if !db_path.exists() {
            self.push_log(format!("Contour DB not found: {}", db_path.display()));
            return;
        }
        let Ok(conn) = rusqlite::Connection::open_with_flags(&db_path, OpenFlags::SQLITE_OPEN_READ_ONLY) else {
            self.push_log("Failed to open contour DB for scanning.".to_owned());
            return;
        };
        let Ok(mut stmt) = conn.prepare(
            "SELECT zoom_bucket, lat_bucket, lon_bucket FROM contour_tile_manifest"
        ) else {
            self.push_log("Failed to query contour_tile_manifest.".to_owned());
            return;
        };
        let specs = crate::contours::all_specs();
        let mut tiles = Vec::new();
        if let Ok(rows) = stmt.query_map([], |row| {
            Ok((row.get::<_, i32>(0)?, row.get::<_, i32>(1)?, row.get::<_, i32>(2)?))
        }) {
            for row in rows.flatten() {
                let (zoom_bucket, lat_bucket, lon_bucket) = row;
                if let Some(spec) = specs.iter().find(|s| s.zoom_bucket == zoom_bucket) {
                    let step = spec.half_extent_deg * 0.45;
                    let center_lat = lat_bucket as f32 * step;
                    let center_lon = lon_bucket as f32 * step;
                    tiles.push((
                        zoom_bucket,
                        center_lat - spec.half_extent_deg,
                        center_lat + spec.half_extent_deg,
                        center_lon - spec.half_extent_deg,
                        center_lon + spec.half_extent_deg,
                    ));
                }
            }
        }
        let count = tiles.len();
        self.cached_tiles = tiles;
        self.push_log(format!("Cache scan: {count} tiles found."));
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
        self.live_tiles.clear();
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
                BuildEvent::TileCompleted(min_lat, max_lat, min_lon, max_lon) => {
                    self.live_tiles.push((min_lat, max_lat, min_lon, max_lon));
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
        // Cell files live in subdirectories (road_cells/, waterway_cells/, etc.),
        // not directly in cache_dir — scan one level of subdirectories.
        if let Ok(top_entries) = fs::read_dir(&cache_dir) {
            for top_entry in top_entries.flatten() {
                let top_path = top_entry.path();
                if !top_path.is_dir() {
                    continue;
                }
                let Ok(sub_entries) = fs::read_dir(&top_path) else {
                    continue;
                };
                for entry in sub_entries.flatten() {
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
                    total_bytes +=
                        metadata.as_ref().map(|meta| meta.len()).unwrap_or_default();
                    files.push((modified, path));
                }
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

        self.inspector.cell_file_count = files.len();
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

    fn scan_lunar_cache(&mut self) {
        let db_path = PathBuf::from(self.lunar_form.lunar_db.trim());
        if !db_path.exists() {
            self.push_log(format!("Lunar cache DB not found: {}", db_path.display()));
            return;
        }
        let Ok(conn) = rusqlite::Connection::open_with_flags(&db_path, OpenFlags::SQLITE_OPEN_READ_ONLY) else {
            self.push_log("Failed to open lunar cache DB.".to_owned());
            return;
        };
        let Ok(mut stmt) = conn.prepare(
            "SELECT zoom_bucket, lat_bucket, lon_bucket FROM contour_tile_manifest"
        ) else {
            self.push_log("Failed to query lunar contour_tile_manifest.".to_owned());
            return;
        };
        let specs = all_lunar_specs();
        let mut tiles = Vec::new();
        if let Ok(rows) = stmt.query_map([], |row| {
            Ok((row.get::<_, i32>(0)?, row.get::<_, i32>(1)?, row.get::<_, i32>(2)?))
        }) {
            for row in rows.flatten() {
                let (zoom_bucket, lat_bucket, lon_bucket) = row;
                if let Some(spec) = specs.iter().find(|s| s.zoom_bucket == zoom_bucket) {
                    let step = spec.half_extent_deg * 0.45;
                    let center_lat = lat_bucket as f32 * step;
                    let center_lon = lon_bucket as f32 * step;
                    tiles.push((
                        zoom_bucket,
                        center_lat - spec.half_extent_deg,
                        center_lat + spec.half_extent_deg,
                        center_lon - spec.half_extent_deg,
                        center_lon + spec.half_extent_deg,
                    ));
                }
            }
        }
        let count = tiles.len();
        self.lunar_cached_tiles = tiles;
        self.push_log(format!("Lunar cache scan: {count} tiles found."));
    }

    fn start_lunar_build(&mut self) {
        if self.active_job.is_some() {
            return;
        }
        let command = match self.lunar_command() {
            Ok(cmd) => cmd,
            Err(e) => { self.push_log(e); return; }
        };
        self.status = "Building Lunar Contours".to_owned();
        self.progress = 0.0;
        self.progress_detail = "Starting lunar build…".to_owned();
        self.lunar_live_tiles.clear();
        self.push_log(format!(
            "Starting lunar build for bbox [{}, {}] x [{}, {}]",
            self.lunar_form.min_lat, self.lunar_form.max_lat,
            self.lunar_form.min_lon, self.lunar_form.max_lon,
        ));
        self.active_job = Some(spawn_job(BuildJob::LunarContours(command)));
    }

    fn lunar_command(&self) -> Result<LunarBuildCommand, String> {
        let f = &self.lunar_form;
        let jp2 = f.jp2_path.trim();
        if jp2.is_empty() {
            return Err("SLDEM JP2 path is required.".to_owned());
        }
        let db = f.lunar_db.trim();
        if db.is_empty() {
            return Err("Lunar cache DB path is required.".to_owned());
        }
        let parse = |label: &str, v: &str| {
            v.parse::<f32>().map_err(|_| format!("Invalid {} '{}'", label, v))
        };
        let min_lat = parse("min lat", &f.min_lat)?;
        let max_lat = parse("max lat", &f.max_lat)?;
        let min_lon = parse("min lon", &f.min_lon)?;
        let max_lon = parse("max lon", &f.max_lon)?;
        if min_lat >= max_lat || min_lon >= max_lon {
            return Err("Invalid bbox: min must be less than max.".to_owned());
        }
        let zoom_buckets: Vec<i32> = f.zoom_buckets.iter().enumerate()
            .filter_map(|(i, &on)| if on { Some(i as i32) } else { None })
            .collect();
        if zoom_buckets.is_empty() {
            return Err("Select at least one zoom level.".to_owned());
        }
        Ok(LunarBuildCommand {
            jp2_path:      PathBuf::from(jp2),
            cache_db_path: PathBuf::from(db),
            tmp_dir:       None,
            min_lat, max_lat, min_lon, max_lon,
            zoom_buckets,
            gdal_bin_dir:  PathBuf::from(f.gdal_bin_dir.trim()),
        })
    }

    fn show_lunar_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("Lunar Contours (SLDEM2015)");
        ui.label("Pre-build lunar_focus_cache.sqlite from the 22 GB SLDEM JP2 file.");
        ui.separator();

        // ── JP2 source ───────────────────────────────────────────────────────
        ui.label("SLDEM2015 JP2 path");
        ui.horizontal(|ui| {
            ui.text_edit_singleline(&mut self.lunar_form.jp2_path);
            if ui.small_button("…").clicked() {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("JP2", &["jp2", "JP2"])
                    .pick_file()
                {
                    self.lunar_form.jp2_path = path.display().to_string();
                }
            }
        });

        // ── Cache DB ─────────────────────────────────────────────────────────
        ui.label("Lunar cache DB (lunar_focus_cache.sqlite)");
        ui.horizontal(|ui| {
            ui.text_edit_singleline(&mut self.lunar_form.lunar_db);
            if ui.small_button("…").clicked() {
                if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                    self.lunar_form.lunar_db = folder
                        .join("lunar_focus_cache.sqlite")
                        .display()
                        .to_string();
                }
            }
        });

        // ── GDAL bin dir ─────────────────────────────────────────────────────
        ui.label("GDAL bin dir (empty = use $PATH)");
        ui.horizontal(|ui| {
            ui.text_edit_singleline(&mut self.lunar_form.gdal_bin_dir);
            if ui.small_button("…").clicked() {
                if let Some(path) = rfd::FileDialog::new().pick_folder() {
                    self.lunar_form.gdal_bin_dir = path.display().to_string();
                }
            }
            if !self.lunar_form.gdal_bin_dir.is_empty() && ui.small_button("✕").clicked() {
                self.lunar_form.gdal_bin_dir.clear();
            }
        });

        // ── Bounding box ─────────────────────────────────────────────────────
        ui.separator();
        ui.label("Bounding Box (clipped to ±60° lat, SLDEM coverage)");
        ui.horizontal(|ui| {
            ui.label("Min Lat");
            ui.add(egui::TextEdit::singleline(&mut self.lunar_form.min_lat).desired_width(60.0));
            ui.label("Max Lat");
            ui.add(egui::TextEdit::singleline(&mut self.lunar_form.max_lat).desired_width(60.0));
        });
        ui.horizontal(|ui| {
            ui.label("Min Lon");
            ui.add(egui::TextEdit::singleline(&mut self.lunar_form.min_lon).desired_width(60.0));
            ui.label("Max Lon");
            ui.add(egui::TextEdit::singleline(&mut self.lunar_form.max_lon).desired_width(60.0));
        });

        // ── Zoom levels ──────────────────────────────────────────────────────
        ui.separator();
        ui.label("Zoom levels to build:");
        let specs = all_lunar_specs();
        ui.horizontal_wrapped(|ui| {
            for (i, spec) in specs.iter().enumerate() {
                let label = format!("Z{} ({}m, {:.1}°)", i, spec.interval_m, spec.half_extent_deg);
                ui.checkbox(&mut self.lunar_form.zoom_buckets[i], label);
            }
        });

        // ── World map ────────────────────────────────────────────────────────
        ui.separator();
        ui.horizontal(|ui| {
            ui.label("Coverage Map");
            if ui.small_button("Scan Cache").clicked() {
                self.scan_lunar_cache();
            }
            if !self.lunar_cached_tiles.is_empty() {
                ui.label(
                    egui::RichText::new(format!("({} cached)", self.lunar_cached_tiles.len()))
                        .color(Color32::from_rgb(0, 180, 160))
                        .small(),
                );
            }
            if !self.lunar_live_tiles.is_empty() {
                ui.label(
                    egui::RichText::new(format!("({} built)", self.lunar_live_tiles.len()))
                        .color(Color32::from_rgb(80, 255, 120))
                        .small(),
                );
            }
        });

        let (response, painter) = ui.allocate_painter(
            egui::Vec2::new(280.0, 140.0),
            egui::Sense::click_and_drag(),
        );
        let rect = response.rect;

        // Background
        painter.rect_filled(rect, 0.0, Color32::from_rgb(4, 6, 18));

        // SLDEM coverage band (±60° lat) — subtle highlight
        {
            let y_top    = lat_to_y(rect,  60.0);
            let y_bottom = lat_to_y(rect, -60.0);
            let band = egui::Rect::from_min_max(
                egui::pos2(rect.left(), y_top),
                egui::pos2(rect.right(), y_bottom),
            );
            painter.rect_filled(band, 0.0, Color32::from_rgba_unmultiplied(30, 60, 120, 50));
        }

        // Cached tiles (dim teal)
        for &(zoom_bucket, min_lat, max_lat, min_lon, max_lon) in &self.lunar_cached_tiles {
            let alpha = (20 + zoom_bucket as u8 * 14).min(110);
            let color = Color32::from_rgba_unmultiplied(0, 180, 160, alpha);
            let tile_rect = egui::Rect::from_min_max(
                egui::pos2(lon_to_x(rect, min_lon), lat_to_y(rect, max_lat)),
                egui::pos2(lon_to_x(rect, max_lon), lat_to_y(rect, min_lat)),
            ).expand(0.5);
            painter.rect_filled(tile_rect, 0.0, color);
        }
        // Live tiles (bright green)
        for &(min_lat, max_lat, min_lon, max_lon) in &self.lunar_live_tiles {
            let tile_rect = egui::Rect::from_min_max(
                egui::pos2(lon_to_x(rect, min_lon), lat_to_y(rect, max_lat)),
                egui::pos2(lon_to_x(rect, max_lon), lat_to_y(rect, min_lat)),
            ).expand(0.5);
            painter.rect_filled(tile_rect, 0.0, Color32::from_rgba_unmultiplied(80, 255, 120, 210));
        }

        // Graticule
        let grid  = Color32::from_rgb(20, 40, 80);
        let equat = Color32::from_rgb(40, 80, 120);
        for lon in [-180i32, -120, -60, 0, 60, 120, 180] {
            let x = lon_to_x(rect, lon as f32);
            painter.line_segment([egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                egui::Stroke::new(1.0, grid));
        }
        for lat in [-90i32, -60, -30, 0, 30, 60, 90] {
            let y = lat_to_y(rect, lat as f32);
            let c = if lat == 0 { equat } else { grid };
            painter.line_segment([egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
                egui::Stroke::new(if lat == 60 || lat == -60 { 1.5 } else { 1.0 }, c));
        }
        // ±60° labels
        for (lat, label) in [(60.0f32, "+60°"), (-60.0, "-60°")] {
            let y = lat_to_y(rect, lat);
            painter.text(egui::pos2(rect.left() + 2.0, y - 7.0),
                egui::Align2::LEFT_TOP, label,
                egui::FontId::proportional(7.5),
                Color32::from_rgb(80, 140, 220),
            );
        }

        // Country outlines
        let coast = Color32::from_rgb(60, 90, 110);
        for ring in coastline_rings() {
            let pts: Vec<egui::Pos2> = ring.iter()
                .map(|[lon, lat]| egui::pos2(lon_to_x(rect, *lon), lat_to_y(rect, *lat)))
                .collect();
            if pts.len() >= 2 {
                painter.add(egui::Shape::line(pts, egui::Stroke::new(0.5, coast)));
            }
        }

        // Current bbox rect
        if let (Ok(min_lat), Ok(max_lat), Ok(min_lon), Ok(max_lon)) = (
            self.lunar_form.min_lat.trim().parse::<f32>(),
            self.lunar_form.max_lat.trim().parse::<f32>(),
            self.lunar_form.min_lon.trim().parse::<f32>(),
            self.lunar_form.max_lon.trim().parse::<f32>(),
        ) {
            let bbox_rect = egui::Rect::from_min_max(
                egui::pos2(lon_to_x(rect, min_lon), lat_to_y(rect, max_lat)),
                egui::pos2(lon_to_x(rect, max_lon), lat_to_y(rect, min_lat)),
            );
            painter.rect_filled(bbox_rect, 0.0, Color32::from_rgba_unmultiplied(220, 160, 30, 38));
            painter.rect_stroke(bbox_rect, 0.0,
                egui::Stroke::new(1.5, Color32::from_rgb(220, 160, 30)),
                egui::StrokeKind::Middle,
            );
        }

        // Drag to set bbox
        if response.drag_started() {
            self.lunar_drag_start = response.hover_pos();
        }
        if response.dragged() {
            if let (Some(start), Some(cur)) = (self.lunar_drag_start, response.hover_pos()) {
                let (lon0, lat0) = (x_to_lon(rect, start.x), y_to_lat(rect, start.y));
                let (lon1, lat1) = (x_to_lon(rect, cur.x),   y_to_lat(rect, cur.y));
                self.lunar_form.min_lat = format!("{:.2}", lat0.min(lat1).max(-60.0));
                self.lunar_form.max_lat = format!("{:.2}", lat0.max(lat1).min(60.0));
                self.lunar_form.min_lon = format!("{:.2}", lon0.min(lon1));
                self.lunar_form.max_lon = format!("{:.2}", lon0.max(lon1));
            }
        }
        if response.drag_stopped() {
            self.lunar_drag_start = None;
        }

        // ── Progress & buttons ───────────────────────────────────────────────
        ui.separator();
        ui.add(
            egui::ProgressBar::new(self.progress)
                .animate(self.active_job.is_some())
                .show_percentage()
                .text(self.progress_detail.clone()),
        );
        ui.horizontal(|ui| {
            let building = self.active_job.is_some();
            if ui.add_enabled(!building, egui::Button::new("Build Lunar Contours")).clicked() {
                self.start_lunar_build();
            }
            if ui.small_button("Scan Cache").clicked() {
                self.scan_lunar_cache();
            }
        });
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
                // ── Tab bar ───────────────────────────────────────────────────
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.active_tab, ActiveTab::OsmContours, "OSM / Contours");
                    ui.selectable_value(&mut self.active_tab, ActiveTab::Lunar, "Lunar (SLDEM)");
                });
                ui.separator();

                if self.active_tab == ActiveTab::Lunar {
                    self.show_lunar_panel(ui);
                    return;
                }

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
                ui.horizontal(|ui| {
                    ui.label("Bounding Box Map");
                    if ui.small_button("Scan Cache").clicked() {
                        self.scan_cache_tiles();
                    }
                    if !self.cached_tiles.is_empty() {
                        ui.label(
                            egui::RichText::new(format!("({} cached)", self.cached_tiles.len()))
                                .color(Color32::from_rgb(0, 180, 160))
                                .small(),
                        );
                    }
                    if !self.live_tiles.is_empty() {
                        ui.label(
                            egui::RichText::new(format!("({} built)", self.live_tiles.len()))
                                .color(Color32::from_rgb(80, 255, 120))
                                .small(),
                        );
                    }
                });

                let (response, painter) = ui
                    .allocate_painter(egui::Vec2::new(280.0, 140.0), egui::Sense::click_and_drag());
                let rect = response.rect;

                // Background
                painter.rect_filled(rect, 0.0, Color32::from_rgb(8, 12, 28));

                // Cached tiles (dim teal — existing cache coverage)
                for &(zoom_bucket, min_lat, max_lat, min_lon, max_lon) in &self.cached_tiles {
                    // Brighter for higher zoom buckets (more detail)
                    let alpha = (25 + zoom_bucket as u8 * 12).min(120);
                    let color = Color32::from_rgba_unmultiplied(0, 180, 160, alpha);
                    let x0 = lon_to_x(rect, min_lon);
                    let x1 = lon_to_x(rect, max_lon);
                    let y0 = lat_to_y(rect, max_lat);
                    let y1 = lat_to_y(rect, min_lat);
                    let tile_rect = egui::Rect::from_min_max(egui::pos2(x0, y0), egui::pos2(x1, y1))
                        .expand(0.5);
                    painter.rect_filled(tile_rect, 0.0, color);
                }

                // Live tiles (bright green — built in this session)
                for &(min_lat, max_lat, min_lon, max_lon) in &self.live_tiles {
                    let color = Color32::from_rgba_unmultiplied(80, 255, 120, 210);
                    let x0 = lon_to_x(rect, min_lon);
                    let x1 = lon_to_x(rect, max_lon);
                    let y0 = lat_to_y(rect, max_lat);
                    let y1 = lat_to_y(rect, min_lat);
                    let tile_rect = egui::Rect::from_min_max(egui::pos2(x0, y0), egui::pos2(x1, y1))
                        .expand(0.5);
                    painter.rect_filled(tile_rect, 0.0, color);
                }

                // Graticule lines
                let graticule_color = Color32::from_rgb(20, 40, 80);
                let equator_color = Color32::from_rgb(40, 80, 120);
                let lons = [
                    -180i32, -150, -120, -90, -60, -30, 0, 30, 60, 90, 120, 150, 180,
                ];
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
                    let color = if lat == 0 {
                        equator_color
                    } else {
                        graticule_color
                    };
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
                let bbox_color = Color32::from_rgb(220, 160, 30);
                let bbox_fill = Color32::from_rgba_unmultiplied(220, 160, 30, 38); // ~15% opacity
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
                    let bbox_rect =
                        egui::Rect::from_min_max(egui::pos2(x0, y0), egui::pos2(x1, y1));
                    painter.rect_filled(bbox_rect, 0.0, bbox_fill);
                    painter.rect_stroke(
                        bbox_rect,
                        0.0,
                        egui::Stroke::new(1.5, bbox_color),
                        egui::StrokeKind::Middle,
                    );
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
                    "Cache cell files: {}",
                    self.inspector.cell_file_count
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
