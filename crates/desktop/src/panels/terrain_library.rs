use crate::city_catalog;
use crate::model::AppModel;
use crate::osm_ingest;
use crate::panels::world_map::contour_asset;
use crate::settings_store;
use crate::terrain_assets;
use crate::terrain_precompute::{self, PrecomputeJobState};
use crate::theme;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

struct ViewportBuildState {
    running: bool,
    cancel: Arc<AtomicBool>,
    last_line: String,
    completed_ok: bool,
}

struct CoverageCache {
    last_osm_refresh: Option<Instant>,
    last_scan: Option<Instant>,
    scanning: bool,
    /// 360×180 grid of covered 1° SRTM / contour cells (row = 89 - lat, col = lon + 180).
    cells: Vec<u8>,
    covered_count: usize,
    viewport_build: ViewportBuildState,
}

fn library_state() -> &'static Mutex<CoverageCache> {
    static STATE: OnceLock<Mutex<CoverageCache>> = OnceLock::new();
    STATE.get_or_init(|| {
        Mutex::new(CoverageCache {
            last_osm_refresh: None,
            last_scan: None,
            scanning: false,
            cells: vec![0u8; 360 * 180],
            covered_count: 0,
            viewport_build: ViewportBuildState {
                running: false,
                cancel: Arc::new(AtomicBool::new(false)),
                last_line: String::new(),
                completed_ok: false,
            },
        })
    })
}

fn spawn_coverage_scan(root: Option<PathBuf>) {
    {
        let mut st = library_state().lock().unwrap_or_else(|p| p.into_inner());
        if st.scanning {
            return;
        }
        st.scanning = true;
    }
    std::thread::spawn(move || {
        let mut grid = vec![0u8; 360 * 180];
        let mut count = 0usize;
        if let Some(srtm_dir) = terrain_assets::find_srtm_root(root.as_deref())
            && let Ok(entries) = std::fs::read_dir(&srtm_dir)
        {
            for entry in entries.flatten() {
                if let Some(name) = entry.file_name().to_str()
                    && let Some((lat, lon)) = parse_hgt_cell(name)
                {
                    let row = (89 - lat).clamp(0, 179) as usize;
                    let col = (lon + 180).clamp(0, 359) as usize;
                    if grid[row * 360 + col] == 0 {
                        grid[row * 360 + col] = 1;
                        count += 1;
                    }
                }
            }
        }
        let mut st = library_state().lock().unwrap_or_else(|p| p.into_inner());
        st.cells = grid;
        st.covered_count = count;
        st.last_scan = Some(Instant::now());
        st.scanning = false;
        crate::app::request_repaint();
    });
}

fn parse_hgt_cell(filename: &str) -> Option<(i32, i32)> {
    let stem = filename.split('.').next()?;
    if stem.len() < 7 {
        return None;
    }
    let bytes = stem.as_bytes();
    let lat_sign = match bytes[0] as char {
        'N' | 'n' => 1,
        'S' | 's' => -1,
        _ => return None,
    };
    let lat: i32 = stem.get(1..3)?.parse().ok()?;
    let lon_sign = match bytes[3] as char {
        'E' | 'e' => 1,
        'W' | 'w' => -1,
        _ => return None,
    };
    let lon: i32 = stem.get(4..7)?.parse().ok()?;
    Some((lat_sign * lat, lon_sign * lon))
}

fn find_cache_builder_exe() -> Option<PathBuf> {
    let exe_dir = settings_store::executable_dir()?;
    let candidate = exe_dir.join(if cfg!(windows) {
        "one-thousand-electric-eye-cache-builder.exe"
    } else {
        "one-thousand-electric-eye-cache-builder"
    });
    candidate.exists().then_some(candidate)
}

fn spawn_viewport_build(model: &mut AppModel) {
    let Some(builder) = find_cache_builder_exe() else {
        model.push_log(
            "Cache builder binary not found beside executable; build one-thousand-electric-eye-cache-builder first."
                .into(),
        );
        return;
    };
    let Some(srtm_root) = terrain_assets::find_srtm_root(model.selected_root.as_deref()) else {
        model.push_log("SRTM source folder not found; configure SRTM root in Settings.".into());
        return;
    };
    let Some(derived) = terrain_assets::find_derived_root(model.selected_root.as_deref()) else {
        model.push_log("Derived root not configured; configure Derived root in Settings.".into());
        return;
    };

    let center = if model.globe_view.local_mode {
        model.globe_view.local_center
    } else {
        model.globe_view.globe_center_latlon()
    };
    let half = if model.globe_view.local_mode {
        crate::panels::world_map::vr_local_extent(model.globe_view.local_zoom).clamp(0.5, 4.0)
    } else {
        (12.0 / model.globe_view.zoom.max(1.0)).clamp(1.0, 5.0)
    };
    let min_lat = (center.lat - half).clamp(-60.0, 60.0);
    let max_lat = (center.lat + half).clamp(-60.0, 60.0);
    let min_lon = (center.lon - half).clamp(-180.0, 180.0);
    let max_lon = (center.lon + half).clamp(-180.0, 180.0);
    let cache_db = derived.join("terrain/srtm_native_v1.sqlite");

    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut st = library_state().lock().unwrap_or_else(|p| p.into_inner());
        if st.viewport_build.running {
            return;
        }
        st.viewport_build.running = true;
        st.viewport_build.cancel = Arc::clone(&cancel);
        st.viewport_build.completed_ok = false;
        st.viewport_build.last_line = format!(
            "Building viewport [{min_lat:.1}..{max_lat:.1}, {min_lon:.1}..{max_lon:.1}]…"
        );
    }

    let worker_cancel = Arc::clone(&cancel);
    std::thread::spawn(move || {
        let mut cmd = Command::new(&builder);
        cmd.args([
            "contours-bbox",
            "--engine",
            "gpu",
            "--srtm-root",
            &srtm_root.display().to_string(),
            "--cache-db",
            &cache_db.display().to_string(),
            "--min-lat",
            &format!("{min_lat:.3}"),
            "--max-lat",
            &format!("{max_lat:.3}"),
            "--min-lon",
            &format!("{min_lon:.3}"),
            "--max-lon",
            &format!("{max_lon:.3}"),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

        let mut ok = false;
        let final_msg = match cmd.spawn() {
            Ok(mut child) => {
                if let Some(stdout) = child.stdout.take() {
                    let reader = BufReader::new(stdout);
                    for line in reader.lines().flatten() {
                        if worker_cancel.load(Ordering::Acquire) {
                            let _ = child.kill();
                            break;
                        }
                        if let Ok(mut st) = library_state().lock() {
                            st.viewport_build.last_line = line;
                        }
                        crate::app::request_repaint();
                    }
                }
                match child.wait() {
                    Ok(status) if status.success() => {
                        ok = true;
                        contour_asset::blast_tile_caches();
                        "Viewport contour cache build complete (caches refreshed).".into()
                    }
                    Ok(status) => format!("Viewport build exited with status {status}"),
                    Err(e) => format!("Viewport build wait failed: {e}"),
                }
            }
            Err(e) => format!("Failed to spawn cache builder: {e}"),
        };

        if let Ok(mut st) = library_state().lock() {
            st.viewport_build.running = false;
            st.viewport_build.completed_ok = ok;
            st.viewport_build.last_line = final_msg;
        }
        crate::app::request_repaint();
    });
}

fn render_coverage_map_and_viewport_builder(ui: &mut egui::Ui, model: &mut AppModel) {
    let should_scan = {
        let st = library_state().lock().unwrap_or_else(|p| p.into_inner());
        st.last_scan
            .is_none_or(|t| t.elapsed() > Duration::from_secs(30))
    };
    if should_scan {
        spawn_coverage_scan(model.selected_root.clone());
    }

    let (covered_count, cells_snapshot, running, last_line) = {
        let st = library_state().lock().unwrap_or_else(|p| p.into_inner());
        (
            st.covered_count,
            st.cells.clone(),
            st.viewport_build.running,
            st.viewport_build.last_line.clone(),
        )
    };

    ui.heading("Local Coverage & Viewport Pre-Build");
    ui.small(format!(
        "SRTM 1° cell coverage on disk: {covered_count} tiles · Amber box shows current camera viewport"
    ));

    let map_w = ui.available_width().clamp(260.0, 460.0);
    let map_h = map_w * 0.5;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(map_w, map_h), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, theme::canvas_background());
    painter.rect_stroke(
        rect,
        4.0,
        egui::Stroke::new(1.0, theme::panel_stroke()),
        egui::StrokeKind::Inside,
    );

    // Draw 2° downsampled dots for fast egui rendering of global 360×180 coverage.
    let cell_col = theme::topo_color().gamma_multiply(0.85);
    let sx = rect.width() / 360.0;
    let sy = rect.height() / 180.0;
    for row in (0..180).step_by(2) {
        for col in (0..360).step_by(2) {
            if cells_snapshot[row * 360 + col] > 0
                || cells_snapshot[row * 360 + (col + 1).min(359)] > 0
            {
                let x = rect.left() + col as f32 * sx;
                let y = rect.top() + row as f32 * sy;
                painter.rect_filled(
                    egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2((sx * 2.0).max(1.5), (sy * 2.0).max(1.5))),
                    0.0,
                    cell_col,
                );
            }
        }
    }

    // Current viewport highlight box
    let center = if model.globe_view.local_mode {
        model.globe_view.local_center
    } else {
        model.globe_view.globe_center_latlon()
    };
    let half = if model.globe_view.local_mode {
        crate::panels::world_map::vr_local_extent(model.globe_view.local_zoom).clamp(0.5, 10.0)
    } else {
        (25.0 / model.globe_view.zoom.max(1.0)).clamp(1.5, 25.0)
    };
    let vx0 = rect.left() + (center.lon - half + 180.0).clamp(0.0, 360.0) * sx;
    let vx1 = rect.left() + (center.lon + half + 180.0).clamp(0.0, 360.0) * sx;
    let vy0 = rect.top() + (90.0 - (center.lat + half)).clamp(0.0, 180.0) * sy;
    let vy1 = rect.top() + (90.0 - (center.lat - half)).clamp(0.0, 180.0) * sy;
    painter.rect_stroke(
        egui::Rect::from_min_max(egui::pos2(vx0, vy0), egui::pos2(vx1.max(vx0 + 4.0), vy1.max(vy0 + 4.0))),
        1.0,
        egui::Stroke::new(1.5, egui::Color32::from_rgb(255, 196, 68)),
        egui::StrokeKind::Outside,
    );

    ui.add_space(6.0);
    ui.horizontal_wrapped(|ui| {
        if ui
            .add_enabled(!running, egui::Button::new("Pre-build Current Viewport"))
            .on_hover_text("Run cache-builder GPU contour generation for the current camera viewport and hot-reload tiles")
            .clicked()
        {
            spawn_viewport_build(model);
        }
        if running && ui.button("Cancel Build").clicked() {
            if let Ok(st) = library_state().lock() {
                st.viewport_build.cancel.store(true, Ordering::Release);
            }
        }
        if running {
            ui.spinner();
        }
    });
    if !last_line.is_empty() {
        ui.small(last_line);
    }
    ui.add_space(8.0);
    ui.separator();
    ui.add_space(6.0);
}

pub(crate) fn tick(ctx: &egui::Context, model: &mut AppModel) {
    // tick() is O(1) when no jobs are queued (reads an AtomicBool).
    terrain_precompute::tick(model.selected_root.as_deref());
    // Only drive the OSM worker when at least one OSM layer is actually
    // wanted by the user.  This prevents background osmium processes from
    // running while all road/water layers are toggled off.
    if model.show_major_roads || model.show_minor_roads || model.show_water {
        osm_ingest::tick(model.selected_root.as_deref());
    }

    if terrain_precompute::has_active_jobs(model.selected_root.as_deref())
        || osm_ingest::has_active_jobs(model.selected_root.as_deref())
    {
        ctx.request_repaint_after(std::time::Duration::from_millis(350));
    }
}

pub(super) fn render(ui: &mut egui::Ui, model: &mut AppModel) {
    super::offline_regions::show(ui, model);
    render_coverage_map_and_viewport_builder(ui, model);
    // Throttle SQLite OSM inventory probes to once every 3 seconds instead of every paint frame.
    let refresh_osm = {
        let mut st = library_state().lock().unwrap_or_else(|p| p.into_inner());
        let due = st
            .last_osm_refresh
            .is_none_or(|t| t.elapsed() > Duration::from_secs(3));
        if due {
            st.last_osm_refresh = Some(Instant::now());
        }
        due
    };
    if refresh_osm && !model.asset_inventory_pending() {
        model.osm_inventory = osm_ingest::OsmInventory::detect_from(model.selected_root.as_deref());
    }
    ui.colored_label(
        theme::text_muted(),
        "Search cities, focus terrain, and queue background contour precompute.",
    );
    ui.add_space(8.0);

    ui.horizontal_wrapped(|ui| {
        ui.label("City Search");
        ui.add_sized(
            [ui.available_width(), 28.0],
            egui::TextEdit::singleline(&mut model.city_filter)
                .hint_text("Type a city or country..."),
        );
    });

    ui.add_space(10.0);
    ui.horizontal_wrapped(|ui| {
        if let Some(city) = model.focused_city() {
            ui.label(format!("Manual focus: {}", city.location_label()));
            if ui.button("Use Event Focus").clicked() {
                model.clear_city_focus();
            }
        } else {
            ui.colored_label(theme::text_muted(), "Manual focus: event-driven");
        }

        ui.separator();
        ui.label(format!("Selected {}", model.selected_city_ids.len()));
    });

    ui.add_space(8.0);
    ui.separator();
    ui.add_space(8.0);

    egui::ScrollArea::vertical()
        .id_salt("terrain-city-results")
        .max_height(280.0)
        .show(ui, |ui| {
            for city in city_catalog::search(&model.city_filter, 80) {
                let mut checked = model.selected_city_ids.contains(city.id.as_str());
                egui::Frame::group(ui.style())
                    .fill(theme::item_fill())
                    .inner_margin(egui::Margin::symmetric(10, 8))
                    .show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            if ui.checkbox(&mut checked, "").changed() {
                                if checked {
                                    model.selected_city_ids.insert(city.id.clone());
                                } else {
                                    model.selected_city_ids.remove(city.id.as_str());
                                }
                            }

                            ui.vertical(|ui| {
                                ui.strong(city.location_label());
                                ui.small(format!(
                                    "{:.4}, {:.4}  ·  pop {:>8}",
                                    city.location.lat, city.location.lon, city.population
                                ));
                            });

                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui.button("Focus").clicked() {
                                        model.focus_city(city.id.as_str());
                                    }
                                },
                            );
                        });
                    });
                ui.add_space(6.0);
            }
        });

    ui.horizontal_wrapped(|ui| {
        if ui.button("Queue Selected").clicked() {
            let selected: Vec<_> = model.selected_city_ids.iter().cloned().collect();
            for city_id in &selected {
                if let Some(city) = city_catalog::by_id(city_id) {
                    terrain_precompute::queue_city(model.selected_root.as_deref(), &city);
                }
            }
            if !selected.is_empty() {
                model.push_log(format!(
                    "Queued terrain precompute for {} city selection(s).",
                    selected.len()
                ));
            }
        }

        if ui.button("Clear Selected").clicked() {
            model.selected_city_ids.clear();
        }
    });

    ui.add_space(12.0);
    ui.separator();
    ui.add_space(8.0);
    ui.heading("OSM");
    ui.colored_label(
        theme::text_muted(),
        "Queue offline planet-derived layers into the shared SQLite tile store.",
    );
    ui.add_space(8.0);

    ui.horizontal_wrapped(|ui| {
    if ui.button("Queue Focus Roads").clicked() {
        match model.terrain_focus_location() {
            Some(focus) => match osm_ingest::queue_focus_roads_import(
                model.selected_root.as_deref(),
                focus,
                20.0,
            ) {
                Ok(true) => {
                    model.push_log(
                        "Queued focused road import for the current terrain focus."
                            .into(),
                    );
                    model.osm_inventory = crate::osm_ingest::OsmInventory::detect_from(
                        model.selected_root.as_deref(),
                    );
                }
                Ok(false) => {
                    model.push_log(
                        "Focused road import was already queued or completed.".into(),
                    );
                }
                Err(error) => {
                    model.push_log(format!("Focused OSM queue failed: {error}"));
                }
            },
            None => {
                model.push_log(
                    "Focused road import requires either a selected event or manual city focus."
                        .into(),
                );
            }
        }
    }

    if ui.button("Queue Global Roads").clicked() {
        match osm_ingest::queue_planet_roads_import(model.selected_root.as_deref()) {
            Ok(true) => {
                model.push_log(
                    "Queued global road bootstrap from planet-latest.osm.pbf.".into(),
                );
                model.osm_inventory = crate::osm_ingest::OsmInventory::detect_from(
                    model.selected_root.as_deref(),
                );
            }
            Ok(false) => {
                model.push_log(
                    "Global road bootstrap was already queued or completed.".into(),
                );
            }
            Err(error) => {
                model.push_log(format!("OSM queue failed: {error}"));
            }
        }
    }

    ui.separator();
    ui.small(format!("Focus: {}", model.terrain_focus_location_name()));

    match osm_ingest::supports_locations_on_ways(model.selected_root.as_deref()) {
        Ok(true) => {
            ui.colored_label(theme::topo_color(), "LocationsOnWays available");
        }
        Ok(false) => {
            ui.colored_label(
                theme::chrome_hot_color(),
                "Planet lacks LocationsOnWays; pure-Rust global roads bootstrap will fail on this source",
            );
        }
        Err(error) => {
            ui.colored_label(theme::text_muted(), error);
        }
    }
});

    let osm_jobs = osm_ingest::snapshots(model.selected_root.as_deref());
    if !osm_jobs.is_empty() {
        ui.add_space(8.0);
        for job in osm_jobs {
            draw_osm_job_row(ui, &job);
        }
    } else {
        ui.small("No OSM ingest jobs queued yet.");
    }

    ui.add_space(12.0);
    ui.separator();
    ui.add_space(8.0);
    ui.heading("Downloads");

    let snapshots = terrain_precompute::snapshots(model.selected_root.as_deref());
    if snapshots.is_empty() {
        ui.colored_label(theme::text_muted(), "No precompute jobs queued yet.");
    } else {
        let (ongoing, completed): (Vec<_>, Vec<_>) = snapshots
            .into_iter()
            .partition(|job| job.state != PrecomputeJobState::Completed);

        if !ongoing.is_empty() {
            ui.label("Ongoing");
            for job in ongoing {
                draw_job_row(ui, &job);
            }
        }

        if !completed.is_empty() {
            ui.add_space(8.0);
            ui.label("Completed");
            for job in completed {
                ui.horizontal_wrapped(|ui| {
                    ui.strong(job.city_label);
                    ui.colored_label(theme::text_muted(), "Ready");
                });
            }
        }
    }
}

fn draw_job_row(ui: &mut egui::Ui, job: &terrain_precompute::PrecomputeJobSnapshot) {
    let progress = if job.total_assets == 0 {
        0.0
    } else {
        (job.ready_assets as f32 / job.total_assets as f32).clamp(0.0, 1.0)
    };

    egui::Frame::group(ui.style())
        .fill(theme::item_fill())
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.strong(job.city_label.as_str());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.colored_label(
                        theme::text_muted(),
                        match job.state {
                            PrecomputeJobState::Queued => "queued",
                            PrecomputeJobState::Running => "running",
                            PrecomputeJobState::Completed => "done",
                        },
                    );
                });
            });

            ui.add(
                egui::ProgressBar::new(progress)
                    .desired_width(ui.available_width())
                    .show_percentage(),
            );
            ui.small(format!(
                "{} / {} buckets ready · {} pending",
                job.ready_assets, job.total_assets, job.pending_assets
            ));
        });
    ui.add_space(6.0);
}

fn draw_osm_job_row(ui: &mut egui::Ui, job: &osm_ingest::OsmJobSnapshot) {
    egui::Frame::group(ui.style())
        .fill(theme::item_fill())
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.strong(job.label.as_str());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.colored_label(theme::text_muted(), job.state.as_str());
                });
            });
            if !job.note.is_empty() {
                ui.small(job.note.as_str());
            }
        });
    ui.add_space(6.0);
}
