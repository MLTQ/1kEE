#[path = "atlas_panel.rs"]
mod atlas_panel;
#[path = "recovery_panel.rs"]
mod recovery_panel;

use crate::args::{BboxCommand, ContoursBboxCommand, PlanetAllCommand};
use crate::job::{BuildEvent, BuildJob, JobHandle, spawn_job_with_wake};
use crate::lunar::{LunarBuildCommand, all_lunar_specs};
use crate::mars::MarsBuildCommand;
use eframe::egui;
use eframe::egui::Color32;
use rusqlite::OpenFlags;
use serde::Deserialize;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::mpsc::{self, TryRecvError};
use std::time::Duration;

#[derive(Clone, Copy, PartialEq)]
enum ActiveTab {
    Recovery,
    Atlas,
    OsmContours,
    Lunar,
    Mars,
}

#[derive(Clone, Copy, PartialEq)]
enum OsmMode {
    Bbox,
    Planet,
}

#[derive(Clone)]
struct LunarForm {
    jp2_path: String,
    lunar_db: String,
    gdal_bin_dir: String,
    min_lat: String,
    max_lat: String,
    min_lon: String,
    max_lon: String,
    /// Per-bucket checkbox state (index = zoom_bucket 0..=4)
    zoom_buckets: [bool; 5],
}

#[derive(Clone)]
struct MarsForm {
    data_root: String,
    cache_db: String,
    gdal_bin_dir: String,
    min_lat: String,
    max_lat: String,
    min_lon: String,
    max_lon: String,
    /// Per-bucket checkbox state (index = zoom_bucket 0..=4)
    zoom_buckets: [bool; 5],
}

// ── Natural Earth 110m country outlines (embedded at compile time) ────────────
const NE_COUNTRIES_JSON: &str = include_str!("../assets/ne_110m_countries.json");
static COASTLINES: OnceLock<Vec<Vec<[f32; 2]>>> = OnceLock::new();

#[derive(Deserialize)]
struct NeCollection {
    features: Vec<NeFeature>,
}
#[derive(Deserialize)]
struct NeFeature {
    geometry: Option<NeGeometry>,
}
#[derive(Deserialize)]
#[serde(tag = "type", content = "coordinates")]
enum NeGeometry {
    Polygon(Vec<Vec<[f32; 2]>>),
    MultiPolygon(Vec<Vec<Vec<[f32; 2]>>>),
    #[serde(other)]
    Other,
}

/// Returns all decimated outer/inner rings from the Natural Earth 110m FeatureCollection.
/// Parsed once via typed serde structs (skipping all unused `properties`) and cached.
fn coastline_rings() -> &'static Vec<Vec<[f32; 2]>> {
    COASTLINES.get_or_init(|| parse_ne_rings(NE_COUNTRIES_JSON))
}

fn decimate_ring(pts: Vec<[f32; 2]>) -> Vec<[f32; 2]> {
    if pts.len() <= 2 {
        return pts;
    }
    const MIN_DEG_SQ: f32 = 0.4 * 0.4;
    let mut out = Vec::with_capacity(pts.len() / 2 + 2);
    out.push(pts[0]);
    let mut last = pts[0];
    for &p in &pts[1..pts.len() - 1] {
        let dx = p[0] - last[0];
        let dy = p[1] - last[1];
        if dx * dx + dy * dy >= MIN_DEG_SQ {
            out.push(p);
            last = p;
        }
    }
    out.push(*pts.last().unwrap());
    out
}

fn parse_ne_rings(json: &str) -> Vec<Vec<[f32; 2]>> {
    let Ok(col) = serde_json::from_str::<NeCollection>(json) else {
        return Vec::new();
    };
    let mut rings = Vec::with_capacity(300);
    for feat in col.features {
        match feat.geometry {
            Some(NeGeometry::Polygon(coords)) => {
                for ring in coords {
                    let dec = decimate_ring(ring);
                    if dec.len() >= 2 {
                        rings.push(dec);
                    }
                }
            }
            Some(NeGeometry::MultiPolygon(polys)) => {
                for poly in polys {
                    for ring in poly {
                        let dec = decimate_ring(ring);
                        if dec.len() >= 2 {
                            rings.push(dec);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    rings
}

const OVERLAY_W: usize = 560;
const OVERLAY_H: usize = 280;

fn blend_over(dst: Color32, src: Color32) -> Color32 {
    let sa = src.a() as u32;
    if sa == 0 {
        return dst;
    }
    if sa == 255 || dst.a() == 0 {
        return src;
    }
    let inv = 255 - sa;
    let da = dst.a() as u32;
    let out_a = sa + (da * inv + 127) / 255;
    if out_a == 0 {
        return Color32::TRANSPARENT;
    }
    let blend_ch = |s: u8, d: u8| -> u8 {
        ((s as u32 * sa + d as u32 * da * inv / 255 + out_a / 2) / out_a).min(255) as u8
    };
    Color32::from_rgba_unmultiplied(
        blend_ch(src.r(), dst.r()),
        blend_ch(src.g(), dst.g()),
        blend_ch(src.b(), dst.b()),
        out_a as u8,
    )
}

fn stamp_rect_into(
    img: &mut egui::ColorImage,
    min_lat: f32,
    max_lat: f32,
    min_lon: f32,
    max_lon: f32,
    color: Color32,
) {
    let x0 = (((min_lon + 180.0) / 360.0 * OVERLAY_W as f32).floor() as i32 - 1)
        .clamp(0, OVERLAY_W as i32 - 1) as usize;
    let x1 = (((max_lon + 180.0) / 360.0 * OVERLAY_W as f32).ceil() as i32 + 1)
        .clamp(1, OVERLAY_W as i32) as usize;
    let y0 = (((90.0 - max_lat) / 180.0 * OVERLAY_H as f32).floor() as i32 - 1)
        .clamp(0, OVERLAY_H as i32 - 1) as usize;
    let y1 = (((90.0 - min_lat) / 180.0 * OVERLAY_H as f32).ceil() as i32 + 1)
        .clamp(1, OVERLAY_H as i32) as usize;
    for y in y0..y1 {
        let row = y * OVERLAY_W;
        for x in x0..x1 {
            img.pixels[row + x] = blend_over(img.pixels[row + x], color);
        }
    }
}

fn rasterize_coastlines(color: Color32) -> egui::ColorImage {
    let mut img = egui::ColorImage::new([OVERLAY_W, OVERLAY_H], Color32::TRANSPARENT);
    let plot = |pixels: &mut [Color32], x: i32, y: i32| {
        if x >= 0 && x < OVERLAY_W as i32 && y >= 0 && y < OVERLAY_H as i32 {
            pixels[y as usize * OVERLAY_W + x as usize] = color;
        }
    };
    for ring in coastline_rings() {
        for pair in ring.windows(2) {
            let mut x0 = ((pair[0][0] + 180.0) / 360.0 * (OVERLAY_W - 1) as f32).round() as i32;
            let mut y0 = ((90.0 - pair[0][1]) / 180.0 * (OVERLAY_H - 1) as f32).round() as i32;
            let x1 = ((pair[1][0] + 180.0) / 360.0 * (OVERLAY_W - 1) as f32).round() as i32;
            let y1 = ((90.0 - pair[1][1]) / 180.0 * (OVERLAY_H - 1) as f32).round() as i32;
            let dx = (x1 - x0).abs();
            let sx = if x0 < x1 { 1 } else { -1 };
            let dy = -(y1 - y0).abs();
            let sy = if y0 < y1 { 1 } else { -1 };
            let mut err = dx + dy;
            loop {
                plot(&mut img.pixels, x0, y0);
                if x0 == x1 && y0 == y1 {
                    break;
                }
                let e2 = 2 * err;
                if e2 >= dy {
                    err += dy;
                    x0 += sx;
                }
                if e2 <= dx {
                    err += dx;
                    y0 += sy;
                }
            }
        }
    }
    img
}

struct CoverageLayer {
    cached_count: usize,
    live_count: usize,
    cached_img: Option<egui::ColorImage>,
    live_img: Option<egui::ColorImage>,
    cached_tex: Option<egui::TextureHandle>,
    live_tex: Option<egui::TextureHandle>,
    cached_dirty: bool,
    live_dirty: bool,
}

impl Default for CoverageLayer {
    fn default() -> Self {
        Self {
            cached_count: 0,
            live_count: 0,
            cached_img: None,
            live_img: None,
            cached_tex: None,
            live_tex: None,
            cached_dirty: false,
            live_dirty: false,
        }
    }
}

impl CoverageLayer {
    fn set_cached(&mut self, count: usize, img: egui::ColorImage) {
        self.cached_count = count;
        self.cached_img = Some(img);
        self.cached_dirty = true;
    }

    fn clear_live(&mut self) {
        self.live_count = 0;
        self.live_img = None;
        self.live_tex = None;
        self.live_dirty = false;
    }

    fn stamp_live(&mut self, min_lat: f32, max_lat: f32, min_lon: f32, max_lon: f32, color: Color32) {
        self.live_count += 1;
        let img = self
            .live_img
            .get_or_insert_with(|| egui::ColorImage::new([OVERLAY_W, OVERLAY_H], Color32::TRANSPARENT));
        stamp_rect_into(img, min_lat, max_lat, min_lon, max_lon, color);
        self.live_dirty = true;
    }

    fn paint(&mut self, ctx: &egui::Context, painter: &egui::Painter, rect: egui::Rect, id: &str) {
        let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        if self.cached_dirty {
            if let Some(img) = &self.cached_img {
                self.cached_tex = Some(ctx.load_texture(
                    format!("{id}_cached"),
                    img.clone(),
                    egui::TextureOptions::NEAREST,
                ));
            }
            self.cached_dirty = false;
        }
        if let Some(tex) = &self.cached_tex {
            painter.image(tex.id(), rect, uv, Color32::WHITE);
        }
        if self.live_dirty {
            if let Some(img) = &self.live_img {
                self.live_tex = Some(ctx.load_texture(
                    format!("{id}_live"),
                    img.clone(),
                    egui::TextureOptions::NEAREST,
                ));
            }
            self.live_dirty = false;
        }
        if let Some(tex) = &self.live_tex {
            painter.image(tex.id(), rect, uv, Color32::WHITE);
        }
    }
}

struct ScanResult {
    tab: ActiveTab,
    outcome: Result<(usize, egui::ColorImage), String>,
}

pub struct BuilderApp {
    form: BuilderForm,
    assets: AssetSelection,
    pub(super) log_lines: VecDeque<String>,
    status: String,
    progress: f32,
    progress_detail: String,
    needs_fresh_working_folder: bool,
    dashboard: crate::builder_dashboard::Dashboard,
    active_job: Option<JobHandle>,
    scan_rx: Option<mpsc::Receiver<ScanResult>>,
    coast_tex_earth: Option<egui::TextureHandle>,
    coast_tex_lunar: Option<egui::TextureHandle>,
    coast_tex_mars: Option<egui::TextureHandle>,
    drag_start: Option<egui::Pos2>,
    osm_mode: OsmMode,
    earth_coverage: CoverageLayer,
    // ── Lunar tab ─────────────────────────────────────────────────────────────
    active_tab: ActiveTab,
    atlas_form: atlas_panel::Form,
    recovery_form: crate::contour_recovery::Command,
    lunar_form: LunarForm,
    lunar_drag_start: Option<egui::Pos2>,
    lunar_coverage: CoverageLayer,
    // ── Mars tab ──────────────────────────────────────────────────────────────
    mars_form: MarsForm,
    mars_drag_start: Option<egui::Pos2>,
    mars_coverage: CoverageLayer,
    /// Which tab started the active build job (for routing TileCompleted events).
    active_job_tab: Option<ActiveTab>,
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
    // Planet-all specific
    planet_tmp_dir: String, // empty = use cache_dir/.planet_build
    compact_nodes: bool,
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
    power: bool,
    rail: bool,
    pipeline: bool,
    aeroway: bool,
    military: bool,
    comm: bool,
    industrial: bool,
    port: bool,
    government: bool,
    surveillance: bool,
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
                    .join(tile_archive::contour_grid::SRTM_NATIVE_DB_NAME)
                    .display()
                    .to_string(),
                gdal_bin_dir: String::new(),
                use_native_engine: true,
                min_lat: "37.60".to_owned(),
                max_lat: "37.90".to_owned(),
                min_lon: "-122.60".to_owned(),
                max_lon: "-122.20".to_owned(),
                margin_deg: "0.08".to_owned(),
                planet_tmp_dir: String::new(),
                compact_nodes: false,
            },
            assets: AssetSelection {
                roads: true,
                water: false,
                buildings: false,
                trees: false,
                admin: false,
                power: false,
                rail: false,
                pipeline: false,
                aeroway: false,
                military: false,
                comm: false,
                industrial: false,
                port: false,
                government: false,
                surveillance: false,
            },
            log_lines: VecDeque::from(["Ready.".to_owned()]),
            status: "Idle".to_owned(),
            progress: 0.0,
            progress_detail: "No active build".to_owned(),
            needs_fresh_working_folder: false,
            dashboard: Default::default(),
            active_job: None,
            scan_rx: None,
            coast_tex_earth: None,
            coast_tex_lunar: None,
            coast_tex_mars: None,
            drag_start: None,
            osm_mode: OsmMode::Bbox,
            earth_coverage: Default::default(),
            active_tab: ActiveTab::OsmContours,
            atlas_form: Default::default(),
            recovery_form: Default::default(),
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
                max_lat: "60.0".to_owned(),
                min_lon: "-180.0".to_owned(),
                max_lon: "180.0".to_owned(),
                zoom_buckets: [true, true, false, false, false],
            },
            lunar_drag_start: None,
            lunar_coverage: Default::default(),
            mars_form: MarsForm {
                data_root: String::new(),
                cache_db: std::env::current_dir()
                    .unwrap_or_else(|_| PathBuf::from("."))
                    .join("Derived")
                    .join("terrain")
                    .join("mars_ctx_cache.sqlite")
                    .display()
                    .to_string(),
                gdal_bin_dir: String::new(),
                min_lat: "-90.0".to_owned(),
                max_lat: "90.0".to_owned(),
                min_lon: "-180.0".to_owned(),
                max_lon: "180.0".to_owned(),
                zoom_buckets: [true, true, false, false, false],
            },
            mars_drag_start: None,
            mars_coverage: Default::default(),
            active_job_tab: None,
        };
        app.refresh_inspector();
        app
    }

    fn start_build(&mut self, ctx: &egui::Context) {
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
        self.dashboard.live.start();
        self.progress = 0.0;
        self.progress_detail = "Starting export…".to_owned();
        self.needs_fresh_working_folder = false;
        self.active_job_tab = Some(ActiveTab::OsmContours);
        self.push_log(format!(
            "Starting export for bbox [{}, {}] x [{}, {}]",
            self.form.min_lat, self.form.max_lat, self.form.min_lon, self.form.max_lon
        ));
        self.active_job = Some(spawn_job_with_wake(
            BuildJob::Bbox(command),
            Some(ctx.clone()),
        ));
    }

    fn spawn_cache_scan(&mut self, tab: ActiveTab, db_path: PathBuf, ctx: &egui::Context) {
        if self.scan_rx.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.scan_rx = Some(rx);
        let wake = ctx.clone();
        std::thread::spawn(move || {
            let outcome = (|| -> Result<(usize, egui::ColorImage), String> {
                if !db_path.exists() {
                    return Err(format!("Cache DB not found: {}", db_path.display()));
                }
                let conn = rusqlite::Connection::open_with_flags(
                    &db_path,
                    OpenFlags::SQLITE_OPEN_READ_ONLY,
                )
                .map_err(|e| format!("Failed to open cache DB: {e}"))?;
                let _ = conn.busy_timeout(Duration::from_secs(2));
                let mut stmt = conn
                    .prepare("SELECT zoom_bucket, lat_bucket, lon_bucket FROM contour_tile_manifest")
                    .map_err(|e| format!("Failed to query contour_tile_manifest: {e}"))?;
                let mut img =
                    egui::ColorImage::new([OVERLAY_W, OVERLAY_H], Color32::TRANSPARENT);
                let mut count = 0usize;
                match tab {
                    ActiveTab::OsmContours => {
                        let specs = crate::contours::all_specs();
                        let rows = stmt
                            .query_map([], |row| {
                                Ok((
                                    row.get::<_, i32>(0)?,
                                    row.get::<_, i32>(1)?,
                                    row.get::<_, i32>(2)?,
                                ))
                            })
                            .map_err(|e| format!("Failed to scan manifest: {e}"))?;
                        for (zoom_bucket, lat_bucket, lon_bucket) in rows.flatten() {
                            if let Some(spec) = usize::try_from(zoom_bucket)
                                .ok()
                                .and_then(|i| specs.get(i))
                                .filter(|s| s.zoom_bucket == zoom_bucket)
                                .or_else(|| specs.iter().find(|s| s.zoom_bucket == zoom_bucket))
                            {
                                let step = spec.half_extent_deg * 0.45;
                                let center_lat = lat_bucket as f32 * step;
                                let center_lon = lon_bucket as f32 * step;
                                let alpha = (25 + zoom_bucket.max(0) as u8 * 12).min(120);
                                stamp_rect_into(
                                    &mut img,
                                    center_lat - step * 0.5,
                                    center_lat + step * 0.5,
                                    center_lon - step * 0.5,
                                    center_lon + step * 0.5,
                                    Color32::from_rgba_unmultiplied(0, 180, 160, alpha),
                                );
                                count += 1;
                            }
                        }
                    }
                    ActiveTab::Lunar | ActiveTab::Mars => {
                        let specs = all_lunar_specs();
                        let is_mars = tab == ActiveTab::Mars;
                        let rows = stmt
                            .query_map([], |row| {
                                Ok((
                                    row.get::<_, i32>(0)?,
                                    row.get::<_, i32>(1)?,
                                    row.get::<_, i32>(2)?,
                                ))
                            })
                            .map_err(|e| format!("Failed to scan manifest: {e}"))?;
                        for (zoom_bucket, lat_bucket, lon_bucket) in rows.flatten() {
                            if let Some(spec) = usize::try_from(zoom_bucket)
                                .ok()
                                .and_then(|i| specs.get(i))
                                .filter(|s| s.zoom_bucket == zoom_bucket)
                                .or_else(|| specs.iter().find(|s| s.zoom_bucket == zoom_bucket))
                            {
                                let step = spec.half_extent_deg * 0.45;
                                let center_lat = lat_bucket as f32 * step;
                                let center_lon = lon_bucket as f32 * step;
                                let alpha = (20 + zoom_bucket.max(0) as u8 * 14).min(110);
                                let color = if is_mars {
                                    Color32::from_rgba_unmultiplied(200, 100, 40, alpha)
                                } else {
                                    Color32::from_rgba_unmultiplied(0, 180, 160, alpha)
                                };
                                stamp_rect_into(
                                    &mut img,
                                    center_lat - spec.half_extent_deg,
                                    center_lat + spec.half_extent_deg,
                                    center_lon - spec.half_extent_deg,
                                    center_lon + spec.half_extent_deg,
                                    color,
                                );
                                count += 1;
                            }
                        }
                    }
                    _ => {}
                }
                Ok((count, img))
            })();
            let _ = tx.send(ScanResult { tab, outcome });
            wake.request_repaint();
        });
    }

    fn poll_cache_scan(&mut self) {
        let Some(rx) = &self.scan_rx else {
            return;
        };
        match rx.try_recv() {
            Ok(result) => {
                self.scan_rx = None;
                match result.outcome {
                    Ok((count, img)) => match result.tab {
                        ActiveTab::Lunar => {
                            self.lunar_coverage.set_cached(count, img);
                            self.push_log(format!("Lunar cache scan: {count} tiles found."));
                        }
                        ActiveTab::Mars => {
                            self.mars_coverage.set_cached(count, img);
                            self.push_log(format!("Mars cache scan: {count} tiles found."));
                        }
                        _ => {
                            self.earth_coverage.set_cached(count, img);
                            self.push_log(format!("Cache scan: {count} tiles found."));
                        }
                    },
                    Err(msg) => self.push_log(msg),
                }
            }
            Err(TryRecvError::Disconnected) => self.scan_rx = None,
            Err(TryRecvError::Empty) => {}
        }
    }

    fn scan_cache_tiles(&mut self, ctx: &egui::Context) {
        let db_path = PathBuf::from(self.form.contour_db.trim());
        self.spawn_cache_scan(ActiveTab::OsmContours, db_path, ctx);
    }

    fn start_contour_build(&mut self, ctx: &egui::Context) {
        if self.active_job.is_some() {
            return;
        }
        let command = match self.contours_command() {
            Ok(cmd) => cmd,
            Err(e) => {
                self.push_log(e);
                return;
            }
        };
        self.status = "Building Contours".to_owned();
        self.dashboard.live.start();
        self.progress = 0.0;
        self.progress_detail = "Starting contour export…".to_owned();
        self.needs_fresh_working_folder = false;
        self.active_job_tab = Some(ActiveTab::OsmContours);
        self.earth_coverage.clear_live();
        self.push_log(format!(
            "Starting contour build for bbox [{}, {}] x [{}, {}]",
            self.form.min_lat, self.form.max_lat, self.form.min_lon, self.form.max_lon,
        ));
        self.active_job = Some(spawn_job_with_wake(
            BuildJob::ContoursBbox(command),
            Some(ctx.clone()),
        ));
    }

    fn contours_command(&self) -> Result<ContoursBboxCommand, String> {
        let parse_num = |label: &str, value: &str| {
            value
                .parse::<f32>()
                .map_err(|_| format!("Invalid {} value '{}'", label, value))
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
            build_power: self.assets.power,
            build_rail: self.assets.rail,
            build_pipeline: self.assets.pipeline,
            build_aeroway: self.assets.aeroway,
            build_military: self.assets.military,
            build_comm: self.assets.comm,
            build_industrial: self.assets.industrial,
            build_port: self.assets.port,
            build_government: self.assets.government,
            build_surveillance: self.assets.surveillance,
        };
        if command.min_lat >= command.max_lat || command.min_lon >= command.max_lon {
            return Err("Invalid bbox: minimums must be less than maximums.".to_owned());
        }
        Ok(command)
    }

    fn planet_command(&self) -> Result<PlanetAllCommand, String> {
        let planet_path = PathBuf::from(self.form.planet_path.trim());
        if self.form.planet_path.trim().is_empty() {
            return Err("Planet PBF path is required.".to_owned());
        }
        let out_dir = PathBuf::from(self.form.cache_dir.trim());
        if self.form.cache_dir.trim().is_empty() {
            return Err("Cache / output directory is required.".to_owned());
        }
        let tmp_dir = {
            let t = self.form.planet_tmp_dir.trim();
            if t.is_empty() {
                out_dir.join(".planet_build")
            } else {
                PathBuf::from(t)
            }
        };
        let srtm_root = {
            let t = self.form.srtm_root.trim();
            if t.is_empty() { None } else { Some(PathBuf::from(t)) }
        };
        Ok(PlanetAllCommand {
            planet_path,
            out_dir,
            tmp_dir,
            node_storage: if self.form.compact_nodes {
                crate::args::NodeStorage::IndexedPbf
            } else {
                crate::args::NodeStorage::Flat
            },
            srtm_root,
            build_roads: self.assets.roads,
            build_waterways: self.assets.water,
            build_buildings: self.assets.buildings,
            build_trees: self.assets.trees,
            build_admin: self.assets.admin,
            build_power: self.assets.power,
            build_rail: self.assets.rail,
            build_pipeline: self.assets.pipeline,
            build_aeroway: self.assets.aeroway,
            build_military: self.assets.military,
            build_comm: self.assets.comm,
            build_industrial: self.assets.industrial,
            build_port: self.assets.port,
            build_government: self.assets.government,
            build_surveillance: self.assets.surveillance,
        })
    }

    fn start_archive_pack(&mut self, ctx: &egui::Context) {
        if self.active_job.is_some() {
            return;
        }
        let osm = PathBuf::from(self.form.cache_dir.trim());
        if !osm.is_dir() {
            self.push_log("Choose an existing vector cache directory first.".to_owned());
            return;
        }
        let Some(out) = rfd::FileDialog::new()
            .set_file_name(tile_archive::FILE_NAME)
            .set_directory(osm.parent().unwrap_or(&osm))
            .add_filter("1kEE archive", &["1ka"])
            .save_file()
        else {
            return;
        };
        self.status = "Packing vector archive".to_owned();
        self.dashboard.live.start();
        self.progress = 0.0;
        self.progress_detail =
            "Packing existing cached geometry; completed cells appear in the log.".to_owned();
        self.needs_fresh_working_folder = false;
        self.active_job_tab = Some(ActiveTab::OsmContours);
        self.active_job = Some(spawn_job_with_wake(
            BuildJob::PackArchive(crate::archive_pack::Command {
                out,
                osm: Some(osm),
                pipelines: None,
                platforms: None,
                terrain: Vec::new(),
            }),
            Some(ctx.clone()),
        ));
    }

    fn start_planet_build(&mut self, ctx: &egui::Context) {
        if self.active_job.is_some() {
            return;
        }
        let command = match self.planet_command() {
            Ok(cmd) => cmd,
            Err(e) => {
                self.push_log(e);
                return;
            }
        };
        self.status = "Building (Planet)".to_owned();
        self.dashboard.live.start();
        self.progress = 0.0;
        self.progress_detail = "Starting planet-all export…".to_owned();
        self.needs_fresh_working_folder = false;
        self.active_job_tab = Some(ActiveTab::OsmContours);
        self.push_log(format!(
            "Starting planet-all build from {}",
            command.planet_path.display()
        ));
        self.active_job = Some(spawn_job_with_wake(
            BuildJob::PlanetAll(command),
            Some(ctx.clone()),
        ));
    }

    fn poll_job(&mut self) {
        let Some(job) = self.active_job.take() else {
            return;
        };

        let mut finished = false;
        loop {
            let event = match job.receiver.try_recv() {
                Ok(event) => event,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    BuildEvent::Finished(Err("Background build worker disconnected".to_owned()))
                }
            };
            match event {
                BuildEvent::AtlasProgress(p) => self.atlas_form.progress = Some(p),
                BuildEvent::Progress(update) => {
                    self.dashboard.live.observe(update.metrics.as_ref());
                    if self.status != update.stage {
                        self.push_log(format!("{} · {}", update.stage, update.message));
                    }
                    self.status = update.stage;
                    self.progress = update.fraction.clamp(0.0, 1.0);
                    self.progress_detail = update.message;
                }
                BuildEvent::Log(line) => {
                    self.push_log(line);
                }
                BuildEvent::TileCompleted(min_lat, max_lat, min_lon, max_lon) => {
                    self.dashboard.live.tiles += 1;
                    match self.active_job_tab {
                        Some(ActiveTab::Lunar) => {
                            self.lunar_coverage.stamp_live(
                                min_lat,
                                max_lat,
                                min_lon,
                                max_lon,
                                Color32::from_rgba_unmultiplied(80, 255, 120, 210),
                            );
                        }
                        Some(ActiveTab::Mars) => {
                            self.mars_coverage.stamp_live(
                                min_lat,
                                max_lat,
                                min_lon,
                                max_lon,
                                Color32::from_rgba_unmultiplied(255, 180, 60, 210),
                            );
                        }
                        _ => {
                            self.earth_coverage.stamp_live(
                                min_lat,
                                max_lat,
                                min_lon,
                                max_lon,
                                Color32::from_rgba_unmultiplied(80, 255, 120, 210),
                            );
                        }
                    }
                }
                BuildEvent::Finished(result) => {
                    self.dashboard.live.finished = Some(std::time::Instant::now());
                    match result {
                        Ok(summary) => {
                            self.status = "Completed".to_owned();
                            self.progress = 1.0;
                            self.needs_fresh_working_folder = false;
                            self.progress_detail = summary.clone();
                            self.push_log(summary);
                        }
                        Err(error) => {
                            self.status = "Failed".to_owned();
                            self.needs_fresh_working_folder =
                                error.to_ascii_lowercase().contains("new working directory");
                            self.progress_detail = error.clone();
                            self.push_log(format!("Build failed: {error}"));
                        }
                    }
                    if !matches!(self.active_job_tab, Some(ActiveTab::Atlas | ActiveTab::Recovery)) {
                        self.refresh_inspector();
                    }
                    self.active_job_tab = None;
                    finished = true;
                    break;
                }
            }
        }
        if !finished {
            self.active_job = Some(job);
        }
    }

    fn refresh_inspector(&mut self) {
        self.dashboard.refresh();
    }

    fn scan_lunar_cache(&mut self, ctx: &egui::Context) {
        let db_path = PathBuf::from(self.lunar_form.lunar_db.trim());
        self.spawn_cache_scan(ActiveTab::Lunar, db_path, ctx);
    }

    fn start_lunar_build(&mut self, ctx: &egui::Context) {
        if self.active_job.is_some() {
            return;
        }
        let command = match self.lunar_command() {
            Ok(cmd) => cmd,
            Err(e) => {
                self.push_log(e);
                return;
            }
        };
        self.status = "Building Lunar Contours".to_owned();
        self.dashboard.live.start();
        self.progress = 0.0;
        self.progress_detail = "Starting lunar build…".to_owned();
        self.needs_fresh_working_folder = false;
        self.active_job_tab = Some(ActiveTab::Lunar);
        self.lunar_coverage.clear_live();
        self.push_log(format!(
            "Starting lunar build for bbox [{}, {}] x [{}, {}]",
            self.lunar_form.min_lat,
            self.lunar_form.max_lat,
            self.lunar_form.min_lon,
            self.lunar_form.max_lon,
        ));
        self.active_job = Some(spawn_job_with_wake(
            BuildJob::LunarContours(command),
            Some(ctx.clone()),
        ));
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
            v.parse::<f32>()
                .map_err(|_| format!("Invalid {} '{}'", label, v))
        };
        let min_lat = parse("min lat", &f.min_lat)?;
        let max_lat = parse("max lat", &f.max_lat)?;
        let min_lon = parse("min lon", &f.min_lon)?;
        let max_lon = parse("max lon", &f.max_lon)?;
        if min_lat >= max_lat || min_lon >= max_lon {
            return Err("Invalid bbox: min must be less than max.".to_owned());
        }
        let zoom_buckets: Vec<i32> = f
            .zoom_buckets
            .iter()
            .enumerate()
            .filter_map(|(i, &on)| if on { Some(i as i32) } else { None })
            .collect();
        if zoom_buckets.is_empty() {
            return Err("Select at least one zoom level.".to_owned());
        }
        Ok(LunarBuildCommand {
            jp2_path: PathBuf::from(jp2),
            cache_db_path: PathBuf::from(db),
            tmp_dir: None,
            min_lat,
            max_lat,
            min_lon,
            max_lon,
            zoom_buckets,
            gdal_bin_dir: PathBuf::from(f.gdal_bin_dir.trim()),
        })
    }

    fn scan_mars_cache(&mut self, ctx: &egui::Context) {
        let db_path = PathBuf::from(self.mars_form.cache_db.trim());
        self.spawn_cache_scan(ActiveTab::Mars, db_path, ctx);
    }

    fn start_mars_build(&mut self, ctx: &egui::Context) {
        if self.active_job.is_some() {
            return;
        }
        let command = match self.mars_command() {
            Ok(cmd) => cmd,
            Err(e) => {
                self.push_log(e);
                return;
            }
        };
        self.status = "Building Mars Contours".to_owned();
        self.dashboard.live.start();
        self.progress = 0.0;
        self.progress_detail = "Starting Mars build…".to_owned();
        self.needs_fresh_working_folder = false;
        self.active_job_tab = Some(ActiveTab::Mars);
        self.mars_coverage.clear_live();
        self.push_log(format!(
            "Starting Mars build for bbox [{}, {}] x [{}, {}]",
            self.mars_form.min_lat,
            self.mars_form.max_lat,
            self.mars_form.min_lon,
            self.mars_form.max_lon,
        ));
        self.active_job = Some(spawn_job_with_wake(
            BuildJob::MarsContours(command),
            Some(ctx.clone()),
        ));
    }

    fn mars_command(&self) -> Result<MarsBuildCommand, String> {
        let f = &self.mars_form;
        let data_root = f.data_root.trim();
        if data_root.is_empty() {
            return Err("Mars data root is required.".to_owned());
        }
        let db = f.cache_db.trim();
        if db.is_empty() {
            return Err("Mars cache DB path is required.".to_owned());
        }
        let parse = |label: &str, v: &str| {
            v.parse::<f32>()
                .map_err(|_| format!("Invalid {} '{}'", label, v))
        };
        let min_lat = parse("min lat", &f.min_lat)?;
        let max_lat = parse("max lat", &f.max_lat)?;
        let min_lon = parse("min lon", &f.min_lon)?;
        let max_lon = parse("max lon", &f.max_lon)?;
        if min_lat >= max_lat || min_lon >= max_lon {
            return Err("Invalid bbox: min must be less than max.".to_owned());
        }
        let zoom_buckets: Vec<i32> = f
            .zoom_buckets
            .iter()
            .enumerate()
            .filter_map(|(i, &on)| if on { Some(i as i32) } else { None })
            .collect();
        if zoom_buckets.is_empty() {
            return Err("Select at least one zoom level.".to_owned());
        }
        Ok(MarsBuildCommand {
            data_root: PathBuf::from(data_root),
            cache_db_path: PathBuf::from(db),
            tmp_dir: None,
            min_lat,
            max_lat,
            min_lon,
            max_lon,
            zoom_buckets,
            gdal_bin_dir: PathBuf::from(f.gdal_bin_dir.trim()),
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
                let label = format!(
                    "Z{} ({}m, {:.1}°)",
                    i, spec.interval_m, spec.half_extent_deg
                );
                ui.checkbox(&mut self.lunar_form.zoom_buckets[i], label);
            }
        });

        // ── World map ────────────────────────────────────────────────────────
        ui.separator();
        ui.horizontal(|ui| {
            ui.label("Coverage Map");
            if ui
                .add_enabled(self.scan_rx.is_none(), egui::Button::new("Scan Cache").small())
                .clicked()
            {
                self.scan_lunar_cache(ui.ctx());
            }
            if self.lunar_coverage.cached_count > 0 {
                ui.label(
                    egui::RichText::new(format!("({} cached)", self.lunar_coverage.cached_count))
                        .color(Color32::from_rgb(0, 180, 160))
                        .small(),
                );
            }
            if self.lunar_coverage.live_count > 0 {
                ui.label(
                    egui::RichText::new(format!("({} built)", self.lunar_coverage.live_count))
                        .color(Color32::from_rgb(80, 255, 120))
                        .small(),
                );
            }
        });

        let (response, painter) =
            ui.allocate_painter(egui::Vec2::new(280.0, 140.0), egui::Sense::click_and_drag());
        let rect = response.rect;

        // Background
        painter.rect_filled(rect, 0.0, Color32::from_rgb(4, 6, 18));

        // SLDEM coverage band (±60° lat) — subtle highlight
        {
            let y_top = lat_to_y(rect, 60.0);
            let y_bottom = lat_to_y(rect, -60.0);
            let band = egui::Rect::from_min_max(
                egui::pos2(rect.left(), y_top),
                egui::pos2(rect.right(), y_bottom),
            );
            painter.rect_filled(band, 0.0, Color32::from_rgba_unmultiplied(30, 60, 120, 50));
        }

        // Cached + live tiles via baked 2× HiDPI bitmap overlays
        self.lunar_coverage
            .paint(ui.ctx(), &painter, rect, "lunar_cov");

        // Graticule
        let grid = Color32::from_rgb(20, 40, 80);
        let equat = Color32::from_rgb(40, 80, 120);
        for lon in [-180i32, -120, -60, 0, 60, 120, 180] {
            let x = lon_to_x(rect, lon as f32);
            painter.line_segment(
                [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                egui::Stroke::new(1.0_f32, grid),
            );
        }
        for lat in [-90i32, -60, -30, 0, 30, 60, 90] {
            let y = lat_to_y(rect, lat as f32);
            let c = if lat == 0 { equat } else { grid };
            painter.line_segment(
                [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
                egui::Stroke::new(if lat == 60 || lat == -60 { 1.5_f32 } else { 1.0_f32 }, c),
            );
        }
        // ±60° labels
        for (lat, label) in [(60.0f32, "+60°"), (-60.0, "-60°")] {
            let y = lat_to_y(rect, lat);
            painter.text(
                egui::pos2(rect.left() + 2.0, y - 7.0),
                egui::Align2::LEFT_TOP,
                label,
                egui::FontId::proportional(7.5),
                Color32::from_rgb(80, 140, 220),
            );
        }

        // Country outlines (cached static texture)
        let coast_tex = self.coast_tex_lunar.get_or_insert_with(|| {
            ui.ctx().load_texture(
                "coast_lunar",
                rasterize_coastlines(Color32::from_rgb(60, 90, 110)),
                egui::TextureOptions::LINEAR,
            )
        });
        painter.image(
            coast_tex.id(),
            rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            Color32::WHITE,
        );

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
            painter.rect_filled(
                bbox_rect,
                0.0,
                Color32::from_rgba_unmultiplied(220, 160, 30, 38),
            );
            painter.rect_stroke(
                bbox_rect,
                0.0,
                egui::Stroke::new(1.5_f32, Color32::from_rgb(220, 160, 30)),
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
                let (lon1, lat1) = (x_to_lon(rect, cur.x), y_to_lat(rect, cur.y));
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
                .show_percentage()
                .text(&self.status),
        );
        ui.horizontal(|ui| {
            let building = self.active_job.is_some();
            if ui
                .add_enabled(!building, egui::Button::new("Build Lunar Contours"))
                .clicked()
            {
                self.start_lunar_build(ui.ctx());
            }
            if ui
                .add_enabled(self.scan_rx.is_none(), egui::Button::new("Scan Cache").small())
                .clicked()
            {
                self.scan_lunar_cache(ui.ctx());
            }
        });
    }

    fn show_mars_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("Mars Contours (CTX DTMs + MOLA)");
        ui.label("Pre-build mars_ctx_cache.sqlite from CTX stereo-pair DEMs with global MOLA fallback.");
        ui.separator();

        // ── Data root ────────────────────────────────────────────────────────
        ui.label("Mars data root (contains mars_data/ and/or MOLA/)");
        ui.horizontal(|ui| {
            ui.text_edit_singleline(&mut self.mars_form.data_root);
            if ui.small_button("…").clicked() {
                if let Some(path) = rfd::FileDialog::new().pick_folder() {
                    self.mars_form.data_root = path.display().to_string();
                }
            }
        });

        // ── Cache DB ─────────────────────────────────────────────────────────
        ui.label("Mars cache DB (desktop: mars_ctx_cache.sqlite)");
        ui.horizontal(|ui| {
            ui.text_edit_singleline(&mut self.mars_form.cache_db);
            if ui.small_button("…").clicked() {
                if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                    self.mars_form.cache_db = folder
                        .join("mars_ctx_cache.sqlite")
                        .display()
                        .to_string();
                }
            }
        });

        // ── GDAL bin dir ─────────────────────────────────────────────────────
        ui.label("GDAL bin dir (empty = use $PATH)");
        ui.horizontal(|ui| {
            ui.text_edit_singleline(&mut self.mars_form.gdal_bin_dir);
            if ui.small_button("…").clicked() {
                if let Some(path) = rfd::FileDialog::new().pick_folder() {
                    self.mars_form.gdal_bin_dir = path.display().to_string();
                }
            }
            if !self.mars_form.gdal_bin_dir.is_empty() && ui.small_button("✕").clicked() {
                self.mars_form.gdal_bin_dir.clear();
            }
        });

        // ── Bounding box ─────────────────────────────────────────────────────
        ui.separator();
        ui.label("Bounding Box (global coverage)");
        ui.horizontal(|ui| {
            ui.label("Min Lat");
            ui.add(egui::TextEdit::singleline(&mut self.mars_form.min_lat).desired_width(60.0));
            ui.label("Max Lat");
            ui.add(egui::TextEdit::singleline(&mut self.mars_form.max_lat).desired_width(60.0));
        });
        ui.horizontal(|ui| {
            ui.label("Min Lon");
            ui.add(egui::TextEdit::singleline(&mut self.mars_form.min_lon).desired_width(60.0));
            ui.label("Max Lon");
            ui.add(egui::TextEdit::singleline(&mut self.mars_form.max_lon).desired_width(60.0));
        });

        // ── Zoom levels ──────────────────────────────────────────────────────
        ui.separator();
        ui.label("Zoom levels to build:");
        let specs = all_lunar_specs();
        ui.horizontal_wrapped(|ui| {
            for (i, spec) in specs.iter().enumerate() {
                let label = format!(
                    "Z{} ({}m, {:.1}°)",
                    i, spec.interval_m, spec.half_extent_deg
                );
                ui.checkbox(&mut self.mars_form.zoom_buckets[i], label);
            }
        });

        // ── World map ────────────────────────────────────────────────────────
        ui.separator();
        ui.horizontal(|ui| {
            ui.label("Coverage Map");
            if ui
                .add_enabled(self.scan_rx.is_none(), egui::Button::new("Scan Cache").small())
                .clicked()
            {
                self.scan_mars_cache(ui.ctx());
            }
            if self.mars_coverage.cached_count > 0 {
                ui.label(
                    egui::RichText::new(format!("({} cached)", self.mars_coverage.cached_count))
                        .color(Color32::from_rgb(200, 120, 60))
                        .small(),
                );
            }
            if self.mars_coverage.live_count > 0 {
                ui.label(
                    egui::RichText::new(format!("({} built)", self.mars_coverage.live_count))
                        .color(Color32::from_rgb(255, 180, 80))
                        .small(),
                );
            }
        });

        let (response, painter) =
            ui.allocate_painter(egui::Vec2::new(280.0, 140.0), egui::Sense::click_and_drag());
        let rect = response.rect;

        // Background — dark reddish to evoke Mars
        painter.rect_filled(rect, 0.0, Color32::from_rgb(18, 8, 6));

        // Cached + live tiles via baked 2× HiDPI bitmap overlays
        self.mars_coverage
            .paint(ui.ctx(), &painter, rect, "mars_cov");

        // Graticule
        let grid = Color32::from_rgb(50, 20, 16);
        let equat = Color32::from_rgb(90, 40, 30);
        for lon in [-180i32, -120, -60, 0, 60, 120, 180] {
            let x = lon_to_x(rect, lon as f32);
            painter.line_segment(
                [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                egui::Stroke::new(1.0_f32, grid),
            );
        }
        for lat in [-90i32, -60, -30, 0, 30, 60, 90] {
            let y = lat_to_y(rect, lat as f32);
            let c = if lat == 0 { equat } else { grid };
            painter.line_segment(
                [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
                egui::Stroke::new(1.0_f32, c),
            );
        }
        // Equator label
        let eq_y = lat_to_y(rect, 0.0);
        painter.text(
            egui::pos2(rect.left() + 2.0, eq_y - 8.0),
            egui::Align2::LEFT_TOP,
            "Equator",
            egui::FontId::proportional(8.0),
            equat,
        );

        // Country outlines (Earth reference — cached static texture)
        let coast_tex = self.coast_tex_mars.get_or_insert_with(|| {
            ui.ctx().load_texture(
                "coast_mars",
                rasterize_coastlines(Color32::from_rgb(50, 30, 24)),
                egui::TextureOptions::LINEAR,
            )
        });
        painter.image(
            coast_tex.id(),
            rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            Color32::WHITE,
        );

        // Current bbox rect
        if let (Ok(min_lat), Ok(max_lat), Ok(min_lon), Ok(max_lon)) = (
            self.mars_form.min_lat.trim().parse::<f32>(),
            self.mars_form.max_lat.trim().parse::<f32>(),
            self.mars_form.min_lon.trim().parse::<f32>(),
            self.mars_form.max_lon.trim().parse::<f32>(),
        ) {
            let bbox_rect = egui::Rect::from_min_max(
                egui::pos2(lon_to_x(rect, min_lon), lat_to_y(rect, max_lat)),
                egui::pos2(lon_to_x(rect, max_lon), lat_to_y(rect, min_lat)),
            );
            painter.rect_filled(
                bbox_rect,
                0.0,
                Color32::from_rgba_unmultiplied(240, 140, 40, 38),
            );
            painter.rect_stroke(
                bbox_rect,
                0.0,
                egui::Stroke::new(1.5_f32, Color32::from_rgb(240, 140, 40)),
                egui::StrokeKind::Middle,
            );
        }

        // Drag to set bbox
        if response.drag_started() {
            self.mars_drag_start = response.hover_pos();
        }
        if response.dragged() {
            if let (Some(start), Some(cur)) = (self.mars_drag_start, response.hover_pos()) {
                let (lon0, lat0) = (x_to_lon(rect, start.x), y_to_lat(rect, start.y));
                let (lon1, lat1) = (x_to_lon(rect, cur.x), y_to_lat(rect, cur.y));
                self.mars_form.min_lat = format!("{:.2}", lat0.min(lat1));
                self.mars_form.max_lat = format!("{:.2}", lat0.max(lat1));
                self.mars_form.min_lon = format!("{:.2}", lon0.min(lon1));
                self.mars_form.max_lon = format!("{:.2}", lon0.max(lon1));
            }
        }
        if response.drag_stopped() {
            self.mars_drag_start = None;
        }

        // ── Progress & buttons ───────────────────────────────────────────────
        ui.separator();
        ui.add(
            egui::ProgressBar::new(self.progress)
                .show_percentage()
                .text(&self.status),
        );
        ui.horizontal(|ui| {
            let building = self.active_job.is_some();
            if ui
                .add_enabled(!building, egui::Button::new("Build Mars Contours"))
                .clicked()
            {
                self.start_mars_build(ui.ctx());
            }
            if ui
                .add_enabled(self.scan_rx.is_none(), egui::Button::new("Scan Cache").small())
                .clicked()
            {
                self.scan_mars_cache(ui.ctx());
            }
        });
    }

    fn push_log(&mut self, line: String) {
        self.log_lines.push_back(line);
        while self.log_lines.len() > 200 {
            self.log_lines.pop_front();
        }
    }
}

impl eframe::App for BuilderApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_job();
        self.poll_cache_scan();
        let building = self.active_job.is_some();
        if !matches!(self.active_tab, ActiveTab::Atlas | ActiveTab::Recovery) {
            self.dashboard.poll(
                &self.form.cache_dir,
                &self.form.planet_tmp_dir,
                self.form.compact_nodes,
                building,
                ctx,
            );
        }
        // Job workers proactively call ctx.request_repaint() (throttled to ~100ms);
        // keep only a 1s fallback tick while building so elapsed counters stay live.
        if building {
            ctx.request_repaint_after(Duration::from_secs(1));
        }

        egui::TopBottomPanel::top("builder_header").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("1kEE Cache Builder");
                ui.separator();
                ui.label(egui::RichText::new("BUILD • INSPECT • RESUME").small().color(Color32::from_rgb(95, 210, 190)));
                ui.separator();
                ui.label(format!("Status: {}", self.status));
            });
        });

        egui::SidePanel::left("builder_controls")
            .resizable(true)
            .default_width(360.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().id_salt("builder_controls_scroll").show(ui, |ui| {
                // ── Tab bar ───────────────────────────────────────────────────
                ui.horizontal_wrapped(|ui| {
                    ui.selectable_value(
                        &mut self.active_tab,
                        ActiveTab::OsmContours,
                        "OSM / Contours",
                    );
                    ui.selectable_value(&mut self.active_tab, ActiveTab::Atlas, "Atlas");
                    ui.selectable_value(&mut self.active_tab, ActiveTab::Lunar, "Lunar (SLDEM)");
                    ui.selectable_value(&mut self.active_tab, ActiveTab::Mars, "Mars");
                    ui.selectable_value(&mut self.active_tab, ActiveTab::Recovery, "Recover terrain");
                });
                ui.separator();

                if self.active_tab == ActiveTab::Recovery { self.show_recovery_controls(ui); return; }
                if self.active_tab == ActiveTab::Atlas { self.show_atlas_panel(ui); return; }
                if self.active_tab == ActiveTab::Lunar {
                    self.show_lunar_panel(ui);
                    return;
                }
                if self.active_tab == ActiveTab::Mars {
                    self.show_mars_panel(ui);
                    return;
                }

                ui.heading("Export");
                if ui
                    .add_enabled(self.active_job.is_none(), egui::Button::new("Pack vector archive…"))
                    .on_hover_text("Convert the selected vector cache directory to a packed runtime archive. Save as Derived/world.1ka; existing files are never overwritten.")
                    .clicked()
                {
                    self.start_archive_pack(ui.ctx());
                }

                ui.horizontal(|ui| {
                    ui.label("Mode:");
                    ui.selectable_value(&mut self.osm_mode, OsmMode::Bbox, "Bounding Box");
                    ui.selectable_value(&mut self.osm_mode, OsmMode::Planet, "Full Planet");
                });
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

                if self.osm_mode == OsmMode::Planet {
                    ui.separator();
                    ui.checkbox(
                        &mut self.form.compact_nodes,
                        "Compact node lookup (experimental — saves disk space)",
                    )
                    .on_hover_text("Index the original PBF, join selected way coordinates once, then build cells. Uses temporary disk space for references and coordinates; resumes each stage from the chosen working folder. Avoids creating the full planet_nodes.bin.");
                    ui.label("Working folder (optional — stores build progress and temporary data)");
                    ui.horizontal(|ui| {
                        ui.text_edit_singleline(&mut self.form.planet_tmp_dir);
                        if ui.small_button("…").clicked() {
                            if let Some(path) = rfd::FileDialog::new().pick_folder() {
                                self.form.planet_tmp_dir = path.display().to_string();
                            }
                        }
                        if !self.form.planet_tmp_dir.is_empty()
                            && ui.small_button("✕").clicked()
                        {
                            self.form.planet_tmp_dir.clear();
                        }
                    });
                    if self.form.planet_tmp_dir.is_empty() {
                        ui.label(
                            egui::RichText::new(format!(
                                "Default: {}/.planet_build",
                                self.form.cache_dir.trim()
                            ))
                            .small()
                            .color(Color32::from_gray(120)),
                        );
                    }
                }

                if self.osm_mode == OsmMode::Planet
                    && self.form.compact_nodes
                    && self.active_job.is_none()
                    && self.status == "Failed"
                    && self.needs_fresh_working_folder
                {
                    ui.add_space(6.0);
                    ui.colored_label(Color32::from_rgb(240, 185, 105), "Saved index cannot be resumed with this source/settings.");
                    ui.label("A fresh working folder rebuilds the index and preserves the existing cache output. This requires another source scan.");
                    if ui.button("Use fresh working folder").clicked() {
                        let current = if self.form.planet_tmp_dir.trim().is_empty() {
                            PathBuf::from(self.form.cache_dir.trim()).join(".planet_build")
                        } else { PathBuf::from(self.form.planet_tmp_dir.trim()) };
                        match crate::build_monitor::fresh_working_folder(&current) {
                            Ok(folder) => {
                                self.form.planet_tmp_dir = folder.display().to_string();
                                self.status = "Ready to rebuild index".into();
                                self.progress = 0.0;
                                self.progress_detail = "Fresh working folder selected. Press Build / resume planet cache to start a new index; existing cache output is preserved.".into();
                                self.push_log(format!("Fresh working folder: {}", folder.display()));
                                self.dashboard.refresh();
                            }
                            Err(error) => { self.push_log(error.clone()); self.progress_detail = error; }
                        }
                    }
                }

                if self.osm_mode == OsmMode::Bbox {
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
                } // end Bbox-only section

                // ── World bbox map (Bbox mode only) ──────────────────────────
                if self.osm_mode == OsmMode::Bbox {
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("Bounding Box Map");
                    if ui
                        .add_enabled(self.scan_rx.is_none(), egui::Button::new("Scan Cache").small())
                        .clicked()
                    {
                        self.scan_cache_tiles(ui.ctx());
                    }
                    if self.earth_coverage.cached_count > 0 {
                        ui.label(
                            egui::RichText::new(format!("({} cached)", self.earth_coverage.cached_count))
                                .color(Color32::from_rgb(0, 180, 160))
                                .small(),
                        );
                    }
                    if self.earth_coverage.live_count > 0 {
                        ui.label(
                            egui::RichText::new(format!("({} built)", self.earth_coverage.live_count))
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

                // Cached + live tiles via baked 2× HiDPI bitmap overlays
                self.earth_coverage
                    .paint(ui.ctx(), &painter, rect, "earth_cov");

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
                        egui::Stroke::new(1.0_f32, graticule_color),
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
                        egui::Stroke::new(1.0_f32, color),
                    );
                }

                // Country outlines (Natural Earth 110m — cached static texture)
                let coast_tex = self.coast_tex_earth.get_or_insert_with(|| {
                    ui.ctx().load_texture(
                        "coast_earth",
                        rasterize_coastlines(Color32::from_rgb(70, 100, 120)),
                        egui::TextureOptions::LINEAR,
                    )
                });
                painter.image(
                    coast_tex.id(),
                    rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    Color32::WHITE,
                );

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
                        egui::Stroke::new(1.5_f32, bbox_color),
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
                } // end Bbox map section

                // ── Assets ────────────────────────────────────────────────────
                ui.separator();
                ui.heading("Assets");
                ui.label("Base layers");
                ui.checkbox(&mut self.assets.roads, "Roads");
                ui.checkbox(&mut self.assets.water, "Waterways");
                ui.checkbox(&mut self.assets.buildings, "Buildings");
                ui.checkbox(&mut self.assets.trees, "Trees / Forest");
                ui.checkbox(&mut self.assets.admin, "Admin Boundaries");
                ui.add_space(4.0);
                ui.label("Infrastructure");
                ui.checkbox(&mut self.assets.power, "Power lines");
                ui.checkbox(&mut self.assets.rail, "Railways");
                ui.checkbox(&mut self.assets.pipeline, "Pipelines");
                ui.checkbox(&mut self.assets.aeroway, "Airports / Aeroways");
                ui.checkbox(&mut self.assets.military, "Military");
                ui.checkbox(&mut self.assets.comm, "Comms towers");
                ui.checkbox(&mut self.assets.industrial, "Industrial areas");
                ui.checkbox(&mut self.assets.port, "Ports / Harbours");
                ui.checkbox(&mut self.assets.government, "Government facilities");
                ui.checkbox(&mut self.assets.surveillance, "Surveillance");

                // ── Terrain / Contours (Bbox mode only) ──────────────────────
                if self.osm_mode == OsmMode::Bbox {
                ui.separator();
                ui.heading("Terrain / Contours");
                ui.label("Native SRTM contour cache file");
                ui.horizontal(|ui| {
                    ui.text_edit_singleline(&mut self.form.contour_db);
                    if ui.small_button("…").clicked() {
                        // Pick a folder — avoids the macOS "Replace?" dialog that
                        // save_file() triggers when the DB already exists.
                        if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                            self.form.contour_db =
                                folder.join(tile_archive::contour_grid::SRTM_NATIVE_DB_NAME).display().to_string();
                        }
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Engine:");
                    ui.radio_value(
                        &mut self.form.use_native_engine,
                        true,
                        "Native (fast, no GDAL)",
                    );
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

                } // end Terrain/Contours bbox-only section

                ui.separator();
                ui.add(
                    egui::ProgressBar::new(self.progress)
                        .show_percentage()
                        .text(&self.status),
                );

                ui.horizontal(|ui| {
                    let building = self.active_job.is_some();
                    if self.osm_mode == OsmMode::Bbox {
                        if ui
                            .add_enabled(!building, egui::Button::new("Build OSM Cache"))
                            .clicked()
                        {
                            self.start_build(ui.ctx());
                        }
                        if ui
                            .add_enabled(!building, egui::Button::new("Build Contours"))
                            .clicked()
                        {
                            self.start_contour_build(ui.ctx());
                        }
                    } else {
                        if ui
                            .add_enabled(!building, egui::Button::new("Build / resume planet cache"))
                            .clicked()
                        {
                            self.start_planet_build(ui.ctx());
                        }
                    }
                    if ui.button("Refresh Inspector").clicked() {
                        self.refresh_inspector();
                    }
                });
                });
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().id_salt("builder_dashboard").show(ui, |ui| {
                if self.active_tab == ActiveTab::Recovery { self.show_recovery_log(ui); return; }
                if self.active_tab == ActiveTab::Atlas { self.show_atlas_dashboard(ui); return; }
                let logs = self.log_lines.make_contiguous();
                self.dashboard.show(ui, &self.status, &self.progress_detail, self.active_job.is_some(), logs, &[
                    ("road", self.assets.roads), ("waterway", self.assets.water),
                    ("building", self.assets.buildings), ("tree", self.assets.trees),
                    ("admin", self.assets.admin), ("power", self.assets.power),
                    ("railway", self.assets.rail), ("pipeline", self.assets.pipeline),
                    ("aeroway", self.assets.aeroway), ("military", self.assets.military),
                    ("comm", self.assets.comm), ("industrial", self.assets.industrial),
                    ("port", self.assets.port), ("government", self.assets.government),
                    ("surveillance", self.assets.surveillance),
                ]);
            });
        });
    }
}
