mod camera;
mod contour_asset;
mod globe_scene;
mod local_terrain_scene;
pub(crate) mod srtm_focus_cache;
mod srtm_stream;
mod terrain_field;
mod terrain_raster;

use crate::model::AppModel;
use crate::osm_ingest;
use crate::theme;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

pub fn render_world_map(ui: &mut egui::Ui, model: &mut AppModel) {
    let panel_frame = egui::Frame::new()
        .fill(theme::section_background())
        .corner_radius(12.0)
        .inner_margin(egui::Margin::same(14));

    panel_frame.show(ui, |ui| {
        if model.globe_view.auto_spin {
            ui.ctx().request_repaint();
        } else if local_terrain_scene::has_pending_cache(model) {
            // Faster repaint while tile pulse animation is running (~30 fps)
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(33));
        } else if globe_srtm_pending(model)
            || (model.show_coastlines
                && contour_asset::global_coastlines_pending(model.selected_root.as_deref()))
            || ((model.show_major_roads || model.show_minor_roads)
                && osm_ingest::has_active_jobs(model.selected_root.as_deref()))
        {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(180));
        }

        let local_terrain_mode = local_terrain_scene::is_active(model);
        ensure_visible_road_layers(model, local_terrain_mode);
        draw_layer_bar(ui, model);

        ui.add_space(8.0);

        let footer_height = if local_terrain_mode { 72.0 } else { 0.0 };
        let desired = egui::vec2(
            ui.available_width().max(480.0),
            (ui.available_height() - footer_height).max(360.0),
        );
        let (response, painter) = ui.allocate_painter(desired, egui::Sense::click_and_drag());
        let rect = response.rect;

        camera::apply_interaction(ui.ctx(), &response, &mut model.globe_view);
        let scene = if local_terrain_mode {
            local_terrain_scene::paint(&painter, rect, model, ui.ctx().input(|input| input.time))
        } else {
            globe_scene::paint(&painter, rect, model, ui.ctx().input(|input| input.time))
        };

        if model.terrain_focus_location().is_some() && !model.cinematic_mode {
            draw_focus_card(ui, model, local_terrain_mode);
        }
        if local_terrain_mode {
            ui.add_space(10.0);
            draw_local_footer(ui, model, scene.beam_elevation_m);
        }

        if response.clicked() && response.drag_delta().length_sq() < 4.0 {
            if let Some(pointer) = response.interact_pointer_pos() {
                if let Some((camera_id, _)) = scene
                    .camera_markers
                    .iter()
                    .find(|(_, marker)| marker.distance(pointer) <= 9.0)
                {
                    model.select_camera(camera_id);
                } else if let Some((event_id, _)) = scene
                    .event_markers
                    .iter()
                    .find(|(_, marker)| marker.distance(pointer) <= 11.0)
                {
                    model.select_event(event_id);
                }
            }
        }

        draw_event_hover_tooltip(ui.ctx(), model, &scene, response.hover_pos());
    });
}

fn draw_layer_bar(ui: &mut egui::Ui, model: &mut AppModel) {
    egui::Frame::new()
        .fill(theme::panel_fill(216))
        .stroke(egui::Stroke::new(1.0, theme::panel_stroke()))
        .corner_radius(10.0)
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.heading("Operations Globe");
                ui.separator();

                // GLOBE / LOCAL mode toggle
                let active_fill = theme::chrome_active_fill();
                let inactive_fill = egui::Color32::TRANSPARENT;
                let active_text = theme::chrome_active_text();
                let inactive_text = theme::text_muted();

                let globe_fill = if !model.globe_view.local_mode {
                    active_fill
                } else {
                    inactive_fill
                };
                let local_fill = if model.globe_view.local_mode {
                    active_fill
                } else {
                    inactive_fill
                };
                let globe_text = if !model.globe_view.local_mode {
                    active_text
                } else {
                    inactive_text
                };
                let local_text = if model.globe_view.local_mode {
                    active_text
                } else {
                    inactive_text
                };

                let globe_btn =
                    egui::Button::new(egui::RichText::new("GLOBE").color(globe_text).small())
                        .fill(globe_fill)
                        .corner_radius(4.0);
                let local_btn =
                    egui::Button::new(egui::RichText::new("LOCAL").color(local_text).small())
                        .fill(local_fill)
                        .corner_radius(4.0);

                if ui.add(globe_btn).clicked() && model.globe_view.local_mode {
                    model.globe_view.local_mode = false;
                }
                if ui.add(local_btn).clicked() && !model.globe_view.local_mode {
                    // Snap local_center to whatever the globe is centered on.
                    model.globe_view.local_center = model.globe_view.globe_center_latlon();
                    model.globe_view.local_mode = true;
                }

                ui.separator();
                ui.colored_label(theme::text_muted(), "Layers");

                ui.checkbox(&mut model.show_event_markers, "Events");
                ui.checkbox(&mut model.show_coastlines, "Coastline");
                ui.checkbox(&mut model.show_graticule, "Graticule");
                if !model.globe_view.local_mode {
                    ui.checkbox(&mut model.show_reticle, "Reticle");
                }
                let major_changed = ui
                    .checkbox(&mut model.show_major_roads, "Major roads")
                    .changed();
                let minor_changed = ui
                    .checkbox(&mut model.show_minor_roads, "Minor roads")
                    .changed();

                if major_changed || minor_changed {
                    // Always clear so the next draw_roads reloads from SQLite
                    // with the correct show-flags, not stale cached geometry.
                    local_terrain_scene::invalidate_road_cache();
                    if model.show_major_roads || model.show_minor_roads {
                        let half_deg = local_terrain_scene::visual_half_extent_for_zoom(
                            model.globe_view.local_zoom,
                        );
                        let r = (half_deg * 69.0 * 1.25).clamp(10.0, 150.0);
                        queue_road_focus_import(
                            model,
                            model.globe_view.local_center,
                            r,
                            "active map viewport",
                        );
                    }
                }

                // Show a compact status note while a road import is running.
                if let Some(note) = osm_ingest::active_job_note() {
                    let short = if note.len() > 42 { &note[..42] } else { &note };
                    ui.colored_label(
                        theme::text_muted(),
                        egui::RichText::new(format!("⟳ {short}…")).small(),
                    );
                }

                if model.selected_event_has_factal_brief() {
                    ui.separator();
                    if ui.button("Brief").clicked() {
                        model.factal_brief_open = true;
                    }
                }

                ui.separator();
                ui.small(model.terrain_focus_location_name());

                // Cinematic toggle — flush right
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let (cin_fill, cin_text) = if model.cinematic_mode {
                        (
                            egui::Color32::from_rgb(160, 100, 20),
                            egui::Color32::from_rgb(255, 210, 80),
                        )
                    } else {
                        (egui::Color32::TRANSPARENT, theme::text_muted())
                    };
                    let cin_btn =
                        egui::Button::new(egui::RichText::new("CINEMATIC").color(cin_text).small())
                            .fill(cin_fill)
                            .corner_radius(4.0);
                    if ui.add(cin_btn).clicked() {
                        model.cinematic_mode = !model.cinematic_mode;
                    }
                });
            });
        });
}

fn draw_event_hover_tooltip(
    ctx: &egui::Context,
    model: &AppModel,
    scene: &globe_scene::GlobeScene,
    hover_pos: Option<egui::Pos2>,
) {
    let Some(pointer) = hover_pos else {
        return;
    };

    let Some((event_id, marker_pos)) = scene
        .event_markers
        .iter()
        .find(|(_, marker)| marker.distance(pointer) <= 12.0)
    else {
        return;
    };

    let Some(event) = model.events.iter().find(|event| event.id == *event_id) else {
        return;
    };

    egui::Area::new("event_hover_tooltip".into())
        .fixed_pos(*marker_pos + egui::vec2(14.0, -8.0))
        .interactable(false)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(theme::panel_fill(238))
                .stroke(egui::Stroke::new(1.0, theme::panel_stroke()))
                .corner_radius(8.0)
                .inner_margin(egui::Margin::same(8))
                .show(ui, |ui| {
                    ui.colored_label(event.severity.color(), event.severity.label());
                    ui.strong(event.title.as_str());
                    ui.small(event.location_name.as_str());
                });
        });
}

fn ensure_visible_road_layers(model: &mut AppModel, local_terrain_mode: bool) {
    if !local_terrain_mode || (!model.show_major_roads && !model.show_minor_roads) {
        return;
    }

    // Rate-limit: only attempt queue checks twice per second.  The actual
    // queue check is now O(1) thanks to in-memory caches, but calling
    // ensure_runtime_store (which opens SQLite) on every frame is still
    // wasteful when nothing has changed.
    static LAST_CHECK: OnceLock<Mutex<Instant>> = OnceLock::new();
    {
        let mut last = LAST_CHECK
            .get_or_init(|| Mutex::new(Instant::now() - std::time::Duration::from_secs(10)))
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if last.elapsed() < std::time::Duration::from_millis(500) {
            return;
        }
        *last = Instant::now();
    }

    if osm_ingest::has_active_jobs(model.selected_root.as_deref()) {
        return;
    }

    // Radius = viewport half-extent in miles, so the import always covers
    // the full visible area regardless of zoom level.
    let half_deg = local_terrain_scene::visual_half_extent_for_zoom(model.globe_view.local_zoom);
    let radius_miles = (half_deg * 69.0 * 1.25).clamp(10.0, 150.0);

    if let Some(focus) = model.terrain_focus_location() {
        queue_road_focus_import(model, focus, radius_miles, "terrain focus");
    }

    let center = model.globe_view.local_center;
    if model
        .terrain_focus_location()
        .map(|focus| (focus.lat - center.lat).abs() > 0.15 || (focus.lon - center.lon).abs() > 0.15)
        .unwrap_or(true)
    {
        queue_road_focus_import(model, center, radius_miles, "map viewport");
    }
}

fn queue_road_focus_import(model: &mut AppModel, point: crate::model::GeoPoint, radius_miles: f32, label: &str) {
    match osm_ingest::queue_focus_roads_import(model.selected_root.as_deref(), point, radius_miles) {
        Ok(true) => {
            model.push_log(format!("Queued focused road import for the {label}."));
            model.osm_inventory =
                osm_ingest::OsmInventory::detect_from(model.selected_root.as_deref());
        }
        Ok(false) => {}
        Err(error) => {
            model.push_log(format!("Focused road import failed: {error}"));
        }
    }
}

fn draw_focus_card(ui: &mut egui::Ui, model: &AppModel, local_terrain_mode: bool) {
    egui::Area::new("focus_card".into())
        .fixed_pos(ui.min_rect().left_top() + egui::vec2(22.0, 72.0))
        .interactable(false)
        .show(ui.ctx(), |ui| {
            egui::Frame::new()
                .fill(theme::panel_fill(230))
                .stroke(egui::Stroke::new(1.0, theme::panel_stroke()))
                .corner_radius(10.0)
                .inner_margin(egui::Margin::same(10))
                .show(ui, |ui| {
                    ui.colored_label(
                        theme::hot_color(),
                        if local_terrain_mode {
                            "LOCAL / 3D CONTOUR STACK"
                        } else {
                            "3D / DARK TOPO / WIREFRAME"
                        },
                    );
                    if let Some(severity) = model.terrain_focus_severity() {
                        ui.colored_label(severity.color(), severity.label());
                    } else {
                        ui.colored_label(theme::topo_color(), "City");
                    }
                    ui.strong(model.terrain_focus_title());
                    ui.label(model.terrain_focus_location_name());
                    ui.small(format!("Source: {}", model.terrain_focus_source()));
                    ui.small(if local_terrain_mode {
                        "Drag to pan | Ctrl/Shift-drag to rotate | scroll to zoom"
                    } else {
                        "Drag to orbit | scroll to zoom"
                    });
                });
        });
}

/// True while SRTM focus tiles for the globe viewport are still being built.
/// Drives repaint so the sphere updates as soon as the background build finishes.
fn globe_srtm_pending(model: &AppModel) -> bool {
    let zoom = model.globe_view.zoom;
    if zoom < 2.0 || model.globe_view.local_mode {
        return false;
    }
    srtm_focus_cache::focus_contour_region_status(
        model.selected_root.as_deref(),
        model.globe_view.local_center,
        zoom,
        0,
    )
    .map(|s| s.pending_assets > 0 || s.ready_assets < s.total_assets)
    .unwrap_or(false)
}

fn draw_local_footer(ui: &mut egui::Ui, model: &mut AppModel, beam_elevation_m: Option<f32>) {
    egui::Frame::new()
        .fill(theme::panel_fill(216))
        .stroke(egui::Stroke::new(1.0, theme::panel_stroke()))
        .corner_radius(10.0)
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(theme::topo_color(), "LAYER SPREAD");
                ui.add_sized(
                    [220.0, 18.0],
                    egui::Slider::new(&mut model.globe_view.local_layer_spread, 0.15..=100.0)
                        .text("Compress / expand")
                        .show_value(true),
                );

                ui.separator();

                // Beam toggle + elevation readout
                ui.checkbox(&mut model.show_beam, "BEAM");
                if let Some(elev) = beam_elevation_m {
                    let cherry = egui::Color32::from_rgb(210, 18, 50);
                    let elev_ft = elev * 3.280_84;
                    ui.colored_label(cherry, format!("{:.0} m / {:.0} ft", elev, elev_ft));
                }

                ui.separator();
                ui.colored_label(theme::hot_color(), "ORANGE");
                ui.label("major contours (50m)");

                ui.separator();
                ui.colored_label(theme::topo_color(), "BLUE");
                ui.label("minor contours");

                if model.show_coastlines {
                    ui.separator();
                    ui.colored_label(theme::contour_color(), "CYAN");
                    ui.label("coastline");
                }

                ui.separator();
                ui.colored_label(egui::Color32::from_rgb(255, 210, 92), "YELLOW");
                ui.label("major roads");

                ui.separator();
                ui.colored_label(egui::Color32::from_rgb(116, 132, 142), "SLATE");
                ui.label("minor roads");

                if local_terrain_scene::is_active(model) {
                    ui.separator();
                    ui.label(format!("Terrain zoom {:.1}x", model.globe_view.local_zoom));
                }
            });
        });
}
