//! Read-only inventory on one background worker; never scan storage during paint.
use crate::build_monitor::{self, Live, Report, bytes, duration};
use egui::{Color32, RichText};
use rayon::prelude::*;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant, SystemTime},
};

const CELL_GRID_WORDS: usize = (180 * 360 + 63) / 64;

#[derive(Clone)]
struct CellGrid {
    bits: Box<[u64; CELL_GRID_WORDS]>,
    count: usize,
}

impl Default for CellGrid {
    fn default() -> Self {
        Self {
            bits: Box::new([0u64; CELL_GRID_WORDS]),
            count: 0,
        }
    }
}

impl CellGrid {
    #[inline]
    fn bit_index(lat: i16, lon: i16) -> Option<usize> {
        if (-90..90).contains(&lat) && (-180..180).contains(&lon) {
            let row = (89 - lat) as usize;
            let col = (lon + 180) as usize;
            Some(row * 360 + col)
        } else {
            None
        }
    }

    fn insert(&mut self, (lat, lon): (i16, i16)) {
        if let Some(idx) = Self::bit_index(lat, lon) {
            let word = &mut self.bits[idx >> 6];
            let mask = 1u64 << (idx & 63);
            if (*word & mask) == 0 {
                *word |= mask;
                self.count += 1;
            }
        }
    }

    #[cfg(test)]
    fn contains(&self, &(lat, lon): &(i16, i16)) -> bool {
        Self::bit_index(lat, lon)
            .is_some_and(|idx| (self.bits[idx >> 6] & (1u64 << (idx & 63))) != 0)
    }

    fn is_empty(&self) -> bool {
        self.count == 0
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.count
    }

    fn to_color_image(&self, bg: Color32, fg: Color32) -> egui::ColorImage {
        let mut pixels = vec![bg; 360 * 180];
        for (word_idx, &word) in self.bits.iter().enumerate() {
            let mut w = word;
            let base = word_idx << 6;
            while w != 0 {
                let tz = w.trailing_zeros() as usize;
                let idx = base + tz;
                if idx < pixels.len() {
                    pixels[idx] = fg;
                }
                w &= w - 1;
            }
        }
        egui::ColorImage {
            size: [360, 180],
            pixels,
        }
    }
}

impl FromIterator<(i16, i16)> for CellGrid {
    fn from_iter<T: IntoIterator<Item = (i16, i16)>>(iter: T) -> Self {
        let mut grid = Self::default();
        for item in iter {
            grid.insert(item);
        }
        grid
    }
}

#[derive(Clone, PartialEq, Eq)]
struct Key {
    output: PathBuf,
    working: PathBuf,
    compact: bool,
}
#[derive(Clone, Default)]
struct Layer {
    files: usize,
    bytes: u64,
    cells: CellGrid,
}
#[derive(Default)]
struct Inventory {
    compact: bool,
    layers: BTreeMap<String, Layer>,
    bytes: u64,
    files: usize,
    legacy: usize,
    report: Option<Report>,
    checkpoint: String,
    checkpoint_summary_cached: String,
    errors: Vec<String>,
}

type DirCache = Arc<Mutex<HashMap<PathBuf, (SystemTime, Layer, usize)>>>;

pub struct Dashboard {
    raw_key: Option<(String, String, bool)>,
    key: Option<Key>,
    key_changed_at: Option<Instant>,
    inventory: Option<Inventory>,
    inventory_rev: u64,
    cell_map_tex: Option<(u64, String, egui::TextureHandle)>,
    pending: Option<(mpsc::Receiver<(Key, Inventory)>, Arc<AtomicBool>)>,
    dir_cache: DirCache,
    refreshed: Option<Instant>,
    pub live: Live,
    layer: String,
    previous: Option<(usize, u64, Instant)>,
    growth: Option<(i64, i128, Duration)>,
}

impl Default for Dashboard {
    fn default() -> Self {
        Self {
            raw_key: None,
            key: None,
            key_changed_at: None,
            inventory: None,
            inventory_rev: 0,
            cell_map_tex: None,
            pending: None,
            dir_cache: Arc::new(Mutex::new(HashMap::new())),
            refreshed: None,
            live: Live::default(),
            layer: String::new(),
            previous: None,
            growth: None,
        }
    }
}

impl Dashboard {
    pub fn refresh(&mut self) {
        self.refreshed = None;
        self.key_changed_at = None;
        if let Ok(mut cache) = self.dir_cache.lock() {
            cache.clear();
        }
    }
    pub fn poll(
        &mut self,
        output: &str,
        working: &str,
        compact: bool,
        building: bool,
        ctx: &egui::Context,
    ) {
        let output_trim = output.trim();
        let working_trim = working.trim();
        let key_changed = self.raw_key.as_ref().is_none_or(|(o, w, c)| {
            o.as_str() != output_trim || w.as_str() != working_trim || *c != compact
        });
        if key_changed {
            let had_key = self.raw_key.is_some();
            let output_pb = PathBuf::from(output_trim);
            let working_pb = if working_trim.is_empty() {
                output_pb.join(".planet_build")
            } else {
                PathBuf::from(working_trim)
            };
            self.raw_key = Some((output_trim.to_owned(), working_trim.to_owned(), compact));
            self.key = Some(Key {
                output: output_pb,
                working: working_pb,
                compact,
            });
            if let Some((_, cancel)) = self.pending.take() {
                cancel.store(true, Ordering::Relaxed);
            }
            self.inventory = None;
            self.cell_map_tex = None;
            self.refreshed = None;
            self.previous = None;
            self.growth = None;
            self.key_changed_at = had_key.then(Instant::now);
        }

        let Some(key) = self.key.clone() else {
            return;
        };

        if let Some((rx, _)) = &self.pending {
            match rx.try_recv() {
                Ok((done, inventory)) => {
                    self.pending = None;
                    if done == key {
                        let now = Instant::now();
                        self.growth = self.previous.map(|(files, size, then)| {
                            (
                                inventory.files as i64 - files as i64,
                                inventory.bytes as i128 - size as i128,
                                now.duration_since(then),
                            )
                        });
                        self.previous = Some((inventory.files, inventory.bytes, now));
                        self.inventory = Some(inventory);
                        self.inventory_rev = self.inventory_rev.wrapping_add(1);
                        self.refreshed = Some(now);
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.pending = None;
                    self.refreshed = Some(Instant::now());
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }

        let debounce_wait = self.key_changed_at.and_then(|changed| {
            Duration::from_millis(350).checked_sub(changed.elapsed())
        });
        if let Some(remaining) = debounce_wait {
            ctx.request_repaint_after(remaining);
            return;
        }
        self.key_changed_at = None;

        // Scan once when unrefreshed (startup, path change, manual Refresh, or build finish),
        // and periodically every 15s only while an active build is writing cells.
        let needs_scan = self.refreshed.is_none()
            || (building
                && self
                    .refreshed
                    .is_some_and(|t| t.elapsed() >= Duration::from_secs(15)));

        if self.pending.is_none() && needs_scan && !output_is_empty(&key) {
            let (tx, rx) = mpsc::channel();
            let cancel = Arc::new(AtomicBool::new(false));
            self.pending = Some((rx, cancel.clone()));
            let dir_cache = self.dir_cache.clone();
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let result = scan_cached(&key, Some(&dir_cache), &cancel);
                if !cancel.load(Ordering::Relaxed) {
                    let _ = tx.send((key, result));
                    ctx.request_repaint();
                }
            });
        }
        if building {
            ctx.request_repaint_after(Duration::from_secs(1));
        }
    }
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        status: &str,
        detail: &str,
        building: bool,
        logs: &[String],
        selected: &[(&str, bool)],
    ) {
        let accent = Color32::from_rgb(95, 210, 190);
        ui.horizontal(|ui| {
            ui.heading("Build overview");
            ui.label(RichText::new(if building { "LIVE" } else { "IDLE" }).color(accent));
            if ui.button("Refresh inventory").clicked() {
                self.refresh();
            }
            if self.pending.is_some() {
                ui.label(RichText::new("Scanning…").small().weak());
            }
        });
        ui.label(
            RichText::new("Source scans, durable progress, and the cache you actually have.")
                .weak(),
        );
        ui.add_space(12.0);
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.label(RichText::new(status).size(20.0).color(accent));
            ui.label(detail);
            ui.add_space(8.0);
            ui.columns(3, |cols| {
                metric(&mut cols[0], "ELAPSED · THIS RUN", &duration(self.live.elapsed()));
                metric(&mut cols[1], "SOURCE SCAN RATE", &self.live.rate().map(|v| format!("{:.1} MB/s", v/1e6)).unwrap_or_else(|| "—".into()));
                metric(&mut cols[2], "EST. TO SCAN EOF", &self.live.eta().map(duration).unwrap_or_else(|| "—".into()));
            });
            if let Some(m) = &self.live.metrics {
                ui.add_space(8.0);
                ui.add(egui::ProgressBar::new(m.scanned_bytes as f32/m.total_bytes.max(1) as f32)
                    .text(format!("{} · {} / {}", m.phase, bytes(m.scanned_bytes), bytes(m.total_bytes))));
                ui.label(format!("Remaining source: {} · Saved position: {} · Since checkpoint: {}", bytes(m.total_bytes.saturating_sub(m.scanned_bytes)), bytes(m.checkpoint_bytes), bytes(m.scanned_bytes.saturating_sub(m.checkpoint_bytes))));
                ui.label(format!("Nodes {}   ·   Ways {}   ·   Selected features {}   ·   Cell writes {}", m.nodes, m.ways, m.features, m.cell_writes));
                if m.batch_total > 0 {
                    ui.label(format!("Current batch: {}/{} blobs complete · Coordinate blocks decoded: {} · Compressed input: {}", m.batch_done, m.batch_total, m.lookup_blocks, bytes(m.lookup_bytes)));
                    ui.small("Lookup counters include rereads and OS-cached input. Batch results are not checkpointed until the batch and required writes finish.");
                }
                ui.small("Way/feature/write counters are for this run. Cell writes can revisit existing cells. ETA covers this source scan only; later passes and writes add time.");
            }
            if self.live.tiles > 0 {
                ui.label(format!("Terrain tiles completed this run: {} · {:.1} tiles/min", self.live.tiles, self.live.tiles as f64 / self.live.elapsed().as_secs_f64().max(1.0) * 60.0));
            }
            if self.live.metrics.is_some() && let Some(advanced) = self.live.advanced {
                ui.small(format!("Last measured progress {}s ago", advanced.elapsed().as_secs()));
            }
            if let Some(updated) = self.live.updated {
                ui.small(format!("Last worker update {}s ago{}", updated.elapsed().as_secs(), if building && updated.elapsed().as_secs()>30 { " · no new progress reported; worker may be busy or waiting on I/O" } else { "" }));
            }
        });
        ui.add_space(12.0);
        let Some(inventory) = &self.inventory else {
            ui.label("Inspecting the selected output folder…");
            return;
        };
        ui.columns(3, |cols| {
            metric(
                &mut cols[0],
                "BINARY CELL FILES",
                &inventory.files.to_string(),
            );
            metric(&mut cols[1], "BINARY CACHE SIZE", &bytes(inventory.bytes));
            metric(
                &mut cols[2],
                "LAYERS PRESENT",
                &inventory.layers.len().to_string(),
            );
        });
        if let Some((files, size, elapsed)) = self.growth {
            ui.small(format!(
                "Inventory change: {files:+} files, {:+.1} MB over {:.0}s · refreshed every 15s",
                size as f64 / 1e6,
                elapsed.as_secs_f64()
            ));
        }
        if let Some(t) = self.refreshed {
            ui.small(format!(
                "Inventory age: {}s · {} legacy GeoJSON files (not added to binary totals)",
                t.elapsed().as_secs(),
                inventory.legacy
            ));
        }
        ui.add_space(8.0);
        ui.label(
            RichText::new("Files present ≠ complete coverage")
                .color(Color32::from_rgb(240, 185, 105)),
        );
        ui.small("Cells are written incrementally. Missing feature totals and complete land coverage cannot be inferred from filenames. The map marks presence only.");
        egui::Grid::new("layer_inventory")
            .striped(true)
            .min_col_width(90.0)
            .show(ui, |ui| {
                ui.strong("Layer");
                ui.strong("Files");
                ui.strong("Size");
                ui.strong("State");
                ui.end_row();
                let names: BTreeSet<_> = inventory
                    .layers
                    .keys()
                    .map(String::as_str)
                    .chain(
                        selected
                            .iter()
                            .filter(|(_, enabled)| *enabled)
                            .map(|(name, _)| *name),
                    )
                    .collect();
                for name in names {
                    let empty = Layer::default();
                    let layer = inventory.layers.get(name).unwrap_or(&empty);
                    if ui.selectable_label(self.layer == name, name).clicked() {
                        self.layer = name.to_owned();
                    }
                    ui.monospace(layer.files.to_string());
                    ui.monospace(bytes(layer.bytes));
                    ui.label(if layer.files == 0 {
                        "No binary cache files"
                    } else {
                        "Present · completeness unknown"
                    });
                    ui.end_row();
                }
            });
        let selected_layer = inventory
            .layers
            .get_key_value(&self.layer)
            .or_else(|| {
                inventory
                    .layers
                    .iter()
                    .find(|(_, layer)| !layer.cells.is_empty())
            });
        if let Some((layer_name, layer)) = selected_layer {
            let map_bg = Color32::from_rgb(12, 23, 30);
            let needs_tex = self.cell_map_tex.as_ref().is_none_or(|(rev, name, _)| {
                *rev != self.inventory_rev || name != layer_name
            });
            if needs_tex {
                let img = layer.cells.to_color_image(map_bg, accent);
                let tex = ui.ctx().load_texture(
                    "builder_cell_presence_map",
                    img,
                    egui::TextureOptions::NEAREST,
                );
                self.cell_map_tex = Some((self.inventory_rev, layer_name.clone(), tex));
            }
            ui.add_space(8.0);
            ui.small("Cell presence · select a layer above");
            let width = ui.available_width().min(800.0);
            let (rect, _) =
                ui.allocate_exact_size(egui::vec2(width, width * 0.38), egui::Sense::hover());
            let painter = ui.painter_at(rect);
            if let Some((_, _, tex)) = &self.cell_map_tex {
                painter.image(
                    tex.id(),
                    rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            } else {
                painter.rect_filled(rect, 6.0, map_bg);
            }
            for lon in (-180..=180).step_by(60) {
                let x = rect.left() + (lon + 180) as f32 / 360.0 * rect.width();
                painter.line_segment(
                    [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                    egui::Stroke::new(1.0_f32, Color32::from_gray(40)),
                );
            }
        }
        ui.add_space(8.0);
        egui::CollapsingHeader::new("Saved build & restart state").default_open(true).show(ui, |ui| {
            if let Some(report) = &inventory.report {
                ui.label(format!("Last recorded outcome: {} · {}", report.state, report.stage));
                ui.label(&report.message);
                ui.small(format!("Recorded output: {}", report.output));
                ui.small(format!("Backend: {} · Source size: {} · Selected layers: {}", report.backend, bytes(report.source_bytes), report.selected_layers.join(", ")));
                if let Some(m) = &report.metrics {
                    ui.small(format!("Last measured {}: {} / {} · {} nodes · {} ways · {} features · {} cell writes", m.phase, bytes(m.scanned_bytes), bytes(m.total_bytes), m.nodes, m.ways, m.features, m.cell_writes));
                }
                ui.small(format!("Updated {}s ago · run duration {} · source {}", build_monitor::unix_now().saturating_sub(report.updated_unix), duration(Duration::from_secs_f64(report.elapsed_secs.max(0.0))), report.source));
                ui.small("A saved ‘running’ record is not proof that a process is still alive. Check its age and the live worker above.");
            } else { ui.label("No status report from an instrumented planet build yet."); }
            if inventory.checkpoint_summary_cached.is_empty() && !inventory.checkpoint.is_empty() {
                ui.label(checkpoint_summary(&inventory.checkpoint, inventory.compact));
            } else {
                ui.label(&inventory.checkpoint_summary_cached);
            }
            egui::CollapsingHeader::new("Checkpoint details").show(ui, |ui| { ui.monospace(&inventory.checkpoint); });
            ui.small("Checkpoint offsets describe resumable scan work, not a guarantee that every cell is complete.");
            for error in &inventory.errors { ui.colored_label(Color32::LIGHT_RED, error); }
        });
        egui::CollapsingHeader::new("Activity log")
            .default_open(true)
            .show(ui, |ui| {
                if ui.small_button("Copy log").clicked() {
                    ui.ctx().copy_text(logs.join("\n"));
                }
                let row_height = ui.text_style_height(&egui::TextStyle::Monospace);
                egui::ScrollArea::vertical()
                    .id_salt("build_log")
                    .max_height(180.0)
                    .stick_to_bottom(true)
                    .show_rows(ui, row_height, logs.len(), |ui, range| {
                        for line in &logs[range] {
                            ui.monospace(line);
                        }
                    });
            });
    }
}
fn output_is_empty(key: &Key) -> bool {
    key.output.as_os_str().is_empty()
}
fn metric(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.label(RichText::new(label).small().weak());
    ui.label(RichText::new(value).size(21.0).strong());
}
fn cell(name: &str) -> Option<(i16, i16)> {
    let mut parts = name.strip_suffix(".1kc")?.rsplit('_');
    let lon = parts.next()?.parse::<i16>().ok()?;
    let lat = parts.next()?.parse::<i16>().ok()?;
    ((-90..90).contains(&lat) && (-180..180).contains(&lon)).then_some((lat, lon))
}
fn checkpoint_summary(text: &str, compact: bool) -> String {
    let fields: BTreeMap<_, _> = text
        .lines()
        .filter_map(|line| line.split_once('='))
        .collect();
    if fields.is_empty() {
        return text.to_owned();
    }
    let number = |key| {
        fields
            .get(key)
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0)
    };
    if compact {
        return format!(
            "Compact backend: node-index state is tracked separately in nodes.sqlite. Feature scan resume: {} (zero means no saved resume position).",
            bytes(number("pass2_offset"))
        );
    }
    let node_state = if fields.get("pass1_complete") == Some(&"true") {
        "Node pass completed"
    } else {
        "Node pass not marked complete"
    };
    let ways = number("pass2_offset");
    format!(
        "{node_state} · {} saved nodes · node resume {}. {}",
        number("pass1_record_count"),
        bytes(number("pass1_offset")),
        if ways > 0 {
            format!("Feature pass has resumable work at {}.", bytes(ways))
        } else {
            "No feature resume offset: this alone cannot distinguish not-started from completed."
                .into()
        }
    )
}
#[cfg(test)]
fn scan(key: &Key) -> Inventory {
    scan_cached(key, None, &AtomicBool::new(false))
}

fn scan_cached(key: &Key, dir_cache: Option<&DirCache>, cancel: &AtomicBool) -> Inventory {
    let mut result = Inventory {
        compact: key.compact,
        ..Default::default()
    };
    let top = match fs::read_dir(&key.output) {
        Ok(v) => v,
        Err(e) => {
            result
                .errors
                .push(format!("Cannot inspect {}: {e}", key.output.display()));
            return result;
        }
    };
    let mut layer_dirs: Vec<(String, PathBuf)> = Vec::new();
    for entry in top {
        if cancel.load(Ordering::Relaxed) {
            return result;
        }
        let entry = match entry {
            Ok(v) => v,
            Err(e) => {
                result.errors.push(e.to_string());
                continue;
            }
        };
        let os_name = entry.file_name();
        let name = os_name.to_string_lossy();
        if name.starts_with('.') || !name.ends_with("_cells") {
            continue;
        }
        layer_dirs.push((name.into_owned(), entry.path()));
    }

    let now_sys = SystemTime::now();
    let per_layer: Vec<(String, Layer, usize, Vec<String>)> = layer_dirs
        .into_par_iter()
        .map(|(name, dir_path)| {
            if cancel.load(Ordering::Relaxed) {
                return (name, Layer::default(), 0, Vec::new());
            }
            let dir_mtime = fs::metadata(&dir_path)
                .ok()
                .and_then(|m| m.modified().ok());
            // Reuse cached layer inventory when directory mtime is stable and >2s old.
            if let (Some(cache), Some(mtime)) = (dir_cache, dir_mtime)
                && now_sys
                    .duration_since(mtime)
                    .is_ok_and(|age| age >= Duration::from_secs(2))
                && let Ok(guard) = cache.lock()
                && let Some((cached_mtime, cached_layer, cached_legacy)) = guard.get(&dir_path)
                && *cached_mtime == mtime
            {
                return (name, cached_layer.clone(), *cached_legacy, Vec::new());
            }

            let mut layer = Layer::default();
            let mut legacy = 0usize;
            let mut errors = Vec::new();
            let entries = match fs::read_dir(&dir_path) {
                Ok(v) => v,
                Err(e) => {
                    errors.push(format!("{name}: {e}"));
                    return (name, layer, legacy, errors);
                }
            };
            for file in entries {
                if cancel.load(Ordering::Relaxed) {
                    return (name, layer, legacy, errors);
                }
                let file = match file {
                    Ok(v) => v,
                    Err(e) => {
                        errors.push(e.to_string());
                        continue;
                    }
                };
                let fname_os = file.file_name();
                let fname = fname_os.to_string_lossy();
                if fname.starts_with('.') {
                    continue;
                }
                if fname.ends_with(".geojson") {
                    legacy += 1;
                } else if fname.ends_with(".1kc") {
                    match file.metadata() {
                        Ok(m) if m.is_file() => {
                            layer.files += 1;
                            layer.bytes += m.len();
                            if let Some(c) = cell(&fname) {
                                layer.cells.insert(c);
                            }
                        }
                        Ok(_) => {}
                        Err(e) => errors.push(e.to_string()),
                    }
                }
            }
            if errors.is_empty()
                && let (Some(cache), Some(mtime)) = (dir_cache, dir_mtime)
                && let Ok(mut guard) = cache.lock()
            {
                guard.insert(dir_path, (mtime, layer.clone(), legacy));
            }
            (name, layer, legacy, errors)
        })
        .collect();

    if cancel.load(Ordering::Relaxed) {
        return result;
    }

    for (name, layer, legacy, errors) in per_layer {
        result.legacy += legacy;
        result.errors.extend(errors);
        result.files += layer.files;
        result.bytes += layer.bytes;
        if layer.files > 0 {
            result
                .layers
                .insert(name.trim_end_matches("_cells").to_owned(), layer);
        }
    }
    let checkpoint = if key.compact {
        key.working.join("indexed-pbf-v1/ways_checkpoint.txt")
    } else {
        key.working.join("checkpoint.txt")
    };
    result.checkpoint = match fs::read_to_string(&checkpoint) {
        Ok(s) => format!("{}\n{s}", checkpoint.display()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            "No scan checkpoint for this working folder/backend.".into()
        }
        Err(e) => {
            result.errors.push(e.to_string());
            "Checkpoint unreadable".into()
        }
    };
    result.checkpoint_summary_cached = checkpoint_summary(&result.checkpoint, result.compact);
    match fs::read(key.working.join("build-status.json")) {
        Ok(bytes) => match serde_json::from_slice(&bytes) {
            Ok(r) => result.report = Some(r),
            Err(e) => result.errors.push(format!("Status report: {e}")),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => result.errors.push(format!("Status report: {e}")),
    }
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inventory_counts_binary_cells_without_claiming_completion() {
        let root = std::env::temp_dir().join(format!("1kee-inventory-{}", std::process::id()));
        fs::create_dir_all(root.join("road_cells")).unwrap();
        fs::write(root.join("road_cells/road_cell_+049_+0023.1kc"), b"partial").unwrap();
        fs::write(
            root.join("road_cells/._road_cell_+049_+0023.1kc"),
            b"macOS metadata",
        )
        .unwrap();
        fs::write(root.join("._road_cells"), b"macOS metadata").unwrap();
        fs::write(root.join("road_cells/road_cell_+049_+0023.geojson"), b"{}").unwrap();
        let data = scan(&Key {
            output: root.clone(),
            working: root.join(".planet_build"),
            compact: false,
        });
        assert_eq!(data.files, 1);
        assert_eq!(data.bytes, 7);
        assert_eq!(data.legacy, 1);
        assert!(data.layers["road"].cells.contains(&(49, 23)));
        assert!(data.report.is_none());
        fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
#[path = "builder_dashboard_preview.rs"]
mod preview;

#[cfg(test)]
mod inspection_tests {
    use super::*;
    #[test]
    #[ignore = "read-only operator cache inventory; requires ONEKEE_CACHE_INSPECT_ROOT"]
    fn inspect_operator_cache() {
        let output = PathBuf::from(std::env::var("ONEKEE_CACHE_INSPECT_ROOT").expect("cache root"));
        let key = Key {
            working: output.join(".planet_build"),
            output,
            compact: false,
        };
        let start = Instant::now();
        let inventory = scan(&key);
        println!(
            "{} binary files, {}, {} layers, {:.2}s",
            inventory.files,
            bytes(inventory.bytes),
            inventory.layers.len(),
            start.elapsed().as_secs_f64()
        );
        for (name, layer) in inventory.layers {
            println!(
                "{name}: {} files, {} coordinate cells, {}",
                layer.files,
                layer.cells.len(),
                bytes(layer.bytes)
            );
        }
        assert!(inventory.errors.is_empty(), "{:?}", inventory.errors);
    }
    #[test]
    fn zero_offset_does_not_claim_complete_and_compact_does_not_claim_missing_nodes() {
        let text = "pass1_complete=false\npass2_offset=0";
        assert!(checkpoint_summary(text, false).contains("cannot distinguish"));
        assert!(!checkpoint_summary(text, true).contains("Node pass not marked complete"));
    }
}
