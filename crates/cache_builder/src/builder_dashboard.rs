//! Read-only inventory on one background worker; never scan storage during paint.
use crate::build_monitor::{self, Live, Report, bytes, duration};
use egui::{Color32, RichText};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::PathBuf,
    sync::mpsc,
    time::{Duration, Instant},
};

#[derive(Clone, PartialEq, Eq)]
struct Key {
    output: PathBuf,
    working: PathBuf,
    compact: bool,
}
#[derive(Default)]
struct Layer {
    files: usize,
    bytes: u64,
    cells: BTreeSet<(i16, i16)>,
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
    errors: Vec<String>,
}
#[derive(Default)]
pub struct Dashboard {
    key: Option<Key>,
    inventory: Option<Inventory>,
    pending: Option<mpsc::Receiver<(Key, Inventory)>>,
    refreshed: Option<Instant>,
    pub live: Live,
    layer: String,
    previous: Option<(usize, u64, Instant)>,
    growth: Option<(i64, i128, Duration)>,
}
impl Dashboard {
    pub fn refresh(&mut self) {
        self.refreshed = None;
    }
    pub fn poll(&mut self, output: &str, working: &str, compact: bool, ctx: &egui::Context) {
        let output = PathBuf::from(output.trim());
        let working = if working.trim().is_empty() {
            output.join(".planet_build")
        } else {
            PathBuf::from(working.trim())
        };
        let key = Key {
            output,
            working,
            compact,
        };
        if self.key.as_ref() != Some(&key) {
            self.key = Some(key.clone());
            self.inventory = None;
            self.refreshed = None;
            self.previous = None;
            self.growth = None;
        }
        if let Some(rx) = &self.pending {
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
        if self.pending.is_none()
            && self
                .refreshed
                .is_none_or(|t| t.elapsed() >= Duration::from_secs(15))
            && !output_is_empty(&key)
        {
            let (tx, rx) = mpsc::channel();
            self.pending = Some(rx);
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let result = scan(&key);
                let _ = tx.send((key, result));
                ctx.request_repaint();
            });
        }
        ctx.request_repaint_after(Duration::from_secs(1));
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
                ui.spinner();
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
        let selected = inventory.layers.get(&self.layer).or_else(|| {
            inventory
                .layers
                .values()
                .find(|layer| !layer.cells.is_empty())
        });
        if let Some(layer) = selected {
            ui.add_space(8.0);
            ui.small("Cell presence · select a layer above");
            let width = ui.available_width().min(800.0);
            let (rect, _) =
                ui.allocate_exact_size(egui::vec2(width, width * 0.38), egui::Sense::hover());
            let painter = ui.painter_at(rect);
            painter.rect_filled(rect, 6.0, Color32::from_rgb(12, 23, 30));
            for lon in (-180..=180).step_by(60) {
                let x = rect.left() + (lon + 180) as f32 / 360.0 * rect.width();
                painter.line_segment(
                    [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                    egui::Stroke::new(1.0_f32, Color32::from_gray(40)),
                );
            }
            for &(lat, lon) in &layer.cells {
                let x = rect.left() + (lon as f32 + 180.0) / 360.0 * rect.width();
                let y = rect.top() + (90.0 - lat as f32 - 1.0) / 180.0 * rect.height();
                painter.rect_filled(
                    egui::Rect::from_min_size(
                        egui::pos2(x, y),
                        egui::vec2(
                            (rect.width() / 360.0).max(1.0),
                            (rect.height() / 180.0).max(1.0),
                        ),
                    ),
                    0.0,
                    accent,
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
            ui.label(checkpoint_summary(&inventory.checkpoint, inventory.compact));
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
                egui::ScrollArea::vertical()
                    .id_salt("build_log")
                    .max_height(180.0)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for line in logs {
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
fn scan(key: &Key) -> Inventory {
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
    for entry in top {
        let entry = match entry {
            Ok(v) => v,
            Err(e) => {
                result.errors.push(e.to_string());
                continue;
            }
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || !name.ends_with("_cells") {
            continue;
        }
        let entries = match fs::read_dir(entry.path()) {
            Ok(v) => v,
            Err(e) => {
                result.errors.push(format!("{name}: {e}"));
                continue;
            }
        };
        let mut layer = Layer::default();
        for file in entries {
            let file = match file {
                Ok(v) => v,
                Err(e) => {
                    result.errors.push(e.to_string());
                    continue;
                }
            };
            if file.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let path = file.path();
            match path.extension().and_then(|s| s.to_str()) {
                Some("geojson") => result.legacy += 1,
                Some("1kc") => match file.metadata() {
                    Ok(m) if m.is_file() => {
                        layer.files += 1;
                        layer.bytes += m.len();
                        if let Some(c) = cell(&file.file_name().to_string_lossy()) {
                            layer.cells.insert(c);
                        }
                    }
                    Ok(_) => {}
                    Err(e) => result.errors.push(e.to_string()),
                },
                _ => {}
            }
        }
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
