mod admin_layer;
mod admin_source;
mod admin_globe;
mod admin_globe_pass;
mod admin_geometry;
mod admin_render;
mod admin_controls;
pub(crate) use admin_controls::controls as admin_controls;
mod building_layer;
mod camera;
mod cell_loader;
mod fire_layer;
mod infra_layer;
mod pipeline_layer;
mod pipeline_capacity;
mod platform_layer;
pub(crate) use platform_layer::controls as platform_controls;
mod pipeline_globe;
mod pipeline_pick;
mod infrastructure_hover;
pub(crate) use pipeline_layer::controls as pipeline_controls;
mod power_layer;
pub(crate) mod contour_asset;
pub(crate) mod contour_pass;
pub(crate) mod gebco_depth_fill;
pub(crate) mod globe_pass;
pub(crate) mod local_contour_pass;
#[path = "world_map/globe_scene/mod.rs"]
mod globe_scene;
mod graticule;
mod layer_import;
mod local_terrain_scene;
mod map_detail_panels;
mod map_tooltips;
mod event_labels;
mod marker_style;
mod road_layer;
#[path = "world_map/srtm_focus_cache/mod.rs"]
pub(crate) mod srtm_focus_cache;
mod srtm_stream;
mod stellar_layer;
mod terrain_field;
mod terrain_raster;
mod layer_snapshot;
mod line_upload;
mod tree_layer;
mod water_layer;
mod waterway_layer;

use crate::arcgis_source;
use crate::flight_tracks;
use crate::model::AppModel;
use crate::moving_tracks;
use crate::osm_ingest;
use crate::theme;

pub(crate) fn invalidate_water_cache_pub() {
    local_terrain_scene::invalidate_water_cache();
}

pub fn render_world_map(ui: &mut egui::Ui, model: &mut AppModel) {
    let panel_frame = if model.obs_view {
        egui::Frame::new().fill(theme::canvas_background())
    } else {
        egui::Frame::new()
            .fill(theme::section_background())
            .corner_radius(12.0)
            .inner_margin(egui::Margin::same(14))
    };

    panel_frame.show(ui, |ui| {
        if model.event_follow.moving() || model.globe_view.auto_spin || model.globe_view.meander_mode {
            ui.ctx().request_repaint();
        } else if local_terrain_scene::has_pending_cache(model) {
            // Faster repaint while tile pulse animation is running (~30 fps)
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(33));
        } else if (model.show_coastlines
                && contour_asset::global_coastlines_pending(model.selected_root.as_deref()))
            || ((model.show_major_roads || model.show_minor_roads || model.show_water)
                && osm_ingest::has_active_jobs(model.selected_root.as_deref()))
            || local_terrain_scene::road_cache_building()
            || local_terrain_scene::water_cache_building()
        {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(180));
        }

        // Refresh AIS vessel cache (non-blocking — spawns background thread
        // if interval elapsed; returns immediately with cached data).
        if model.show_ships && !model.globe_view.local_mode {
            model.tracks = moving_tracks::poll(
                &model.aisstream_api_key,
                model.globe_view.globe_center_latlon(),
                ui.ctx().clone(),
            );
        }

        // Refresh ADS-B flight cache (OpenSky Network, no key required).
        if model.show_flights && !model.globe_view.local_mode {
            model.flights =
                flight_tracks::poll(model.globe_view.globe_center_latlon(), ui.ctx().clone());
        }

        // Poll ArcGIS sources (non-blocking)
        {
            let source_refs: Vec<(String, std::collections::HashSet<u32>)> = model
                .arcgis_sources
                .iter()
                .map(|s| (s.url.clone(), s.enabled_layer_ids.clone()))
                .collect();
            model.arcgis_features = arcgis_source::poll(&source_refs, ui.ctx().clone());
        }

        // Trigger background SLDEM preview + contour builds when Moon Mode is active.
        if model.active_body == crate::model::ActiveBody::Moon {
            srtm_focus_cache::ensure_lunar_preview(model.selected_root.as_deref());
            if srtm_focus_cache::is_lunar_preview_building()
                || srtm_focus_cache::is_lunar_contour_building()
            {
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(250));
            }
        }
        // Keep repainting while Mars contour tiles are being built.
        if model.active_body == crate::model::ActiveBody::Mars
            && srtm_focus_cache::is_mars_contour_building()
        {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(250));
        }

        if !model.obs_view {
            super::map_rail::render(ui, model);
            ui.add_space(8.0);
        }
        let local_terrain_mode = local_terrain_scene::is_active(model);
        layer_import::ensure_visible_road_layers(model, local_terrain_mode);
        layer_import::ensure_visible_water_layers(model, local_terrain_mode);

        let footer_height = if local_terrain_mode && !model.obs_view {
            72.0
        } else {
            0.0
        };
        let desired = egui::vec2(
            ui.available_width().max(if model.obs_view { 1.0 } else { 480.0 }),
            (ui.available_height() - footer_height).max(if model.obs_view { 1.0 } else { 360.0 }),
        );
        let (response, painter) = ui.allocate_painter(desired, egui::Sense::click_and_drag());
        let rect = response.rect;

        if camera::manual_input(ui.ctx(), &response) {
            model.stop_event_follow();
        }
        if !model.event_follow.enabled() {
            camera::apply_interaction(ui.ctx(), &response, &mut model.globe_view);
        }
        infrastructure_hover::begin(
            response.hover_pos().filter(|_| !response.dragged() && !model.obs_view),
        );
        contour_pass::begin_frame();
        painter.add(contour_pass::residency_callback(rect));
        painter.add(local_contour_pass::residency_callback(rect, ui.ctx()));
        if !local_terrain_mode {
            contour_asset::leave_local_view();
        } else {
            contour_asset::globe_residency::set_viewport(None, None);
        }
        let scene = if local_terrain_mode {
            local_terrain_scene::paint(&painter, rect, model, ui.ctx().input(|input| input.time))
        } else {
            globe_scene::paint(&painter, rect, model, ui.ctx().input(|input| input.time))
        };
        contour_pass::end_frame();

        if local_terrain_mode && !model.obs_view {
            ui.add_space(10.0);
            draw_local_footer(ui, model, scene.beam_elevation_m);
        }

        if response.clicked() && response.drag_delta().length_sq() < 4.0 {
            if let Some(pointer) = response.interact_pointer_pos() {
                if let Some((icao24, _)) = scene
                    .flight_markers
                    .iter()
                    .find(|(_, marker)| marker.distance(pointer) <= 11.0)
                {
                    // Toggle selection; trigger metadata fetch on new selection.
                    if model.selected_flight_icao24.as_deref() == Some(icao24.as_str()) {
                        model.selected_flight_icao24 = None;
                    } else {
                        flight_tracks::request_metadata(icao24, ui.ctx().clone());
                        model.selected_flight_icao24 = Some(icao24.clone());
                    }
                } else if let Some((mmsi, _)) = scene
                    .ship_markers
                    .iter()
                    .find(|(_, marker)| marker.distance(pointer) <= 11.0)
                {
                    // Toggle selection: clicking the same ship deselects it.
                    if model.selected_track_mmsi == Some(*mmsi) {
                        model.selected_track_mmsi = None;
                    } else {
                        model.selected_track_mmsi = Some(*mmsi);
                    }
                } else if let Some(marker) = marker_style::pick(&scene.camera_markers, pointer) {
                    model.open_camera_feed(&marker.id);
                } else if let Some(marker) = marker_style::pick(&scene.event_markers, pointer) {
                    model.select_event(&marker.id);
                } else if let Some((src_url, obj_id, _)) = scene
                    .arcgis_feature_markers
                    .iter()
                    .find(|(_, _, marker_pos)| marker_pos.distance(pointer) <= 10.0)
                {
                    let key = (src_url.clone(), *obj_id);
                    if model.selected_arcgis_feature.as_ref() == Some(&key) {
                        model.selected_arcgis_feature = None;
                    } else {
                        model.selected_arcgis_feature = Some(key);
                    }
                }
                // Landing points last: their dots sit under cables and are the
                // least specific target, so any other marker wins a tie.
                else if let Some(&(index, _)) = scene
                    .landing_point_markers
                    .iter()
                    .find(|(_, marker)| marker.distance(pointer) <= 9.0)
                {
                    model.selected_landing_point =
                        (model.selected_landing_point != Some(index)).then_some(index);
                }
            }
        }

        if model.event_follow.enabled() {
            let opacity = model.event_follow.event_label_opacity(ui.ctx().input(|input| input.time));
            event_labels::draw(&painter, rect, model.map_events(), &scene.event_markers, opacity);
        }

        // Persistent tour cards remain in Focus; transient hover/detail panels
        // stay hidden. The app shell renders the open brief/live camera.
        if model.obs_view {
            return;
        }
        super::render_replay_controls(ui, model);
        if response.hover_pos().is_some_and(|pointer| scene.landing_point_markers.iter().any(|(_, p)| p.distance(pointer) <= 9.0)) {
            infrastructure_hover::prefer_landing();
        }
        if !infrastructure_hover::show(ui.ctx()) {
            if !model.event_follow.enabled() {
                map_tooltips::draw_event_hover_tooltip(ui.ctx(), model, &scene, response.hover_pos());
            }
            map_tooltips::draw_camera_hover_tooltip(ui.ctx(), model, &scene, response.hover_pos());
            map_tooltips::draw_ship_hover_tooltip(ui.ctx(), model, &scene, response.hover_pos());
            map_tooltips::draw_flight_hover_tooltip(ui.ctx(), model, &scene, response.hover_pos());
            map_tooltips::draw_landing_hover_tooltip(ui.ctx(), model, &scene, response.hover_pos());
        }
        map_detail_panels::draw_ship_detail_panel(ui.ctx(), model);
        map_detail_panels::draw_flight_detail_panel(ui.ctx(), model);
        map_detail_panels::draw_landing_detail_panel(ui.ctx(), model);
        draw_arcgis_detail_panel(ui.ctx(), model);
        if let Some(hover) = response.hover_pos() {
            if !model.globe_view.local_mode {
                if let Some(geo) = globe_scene::screen_to_latlon(rect, &model.globe_view, hover) {
                    draw_globe_coord_overlay(ui.ctx(), geo, rect);
                }
            }
        }
    });
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
                ui.colored_label(theme::hot_color(), "MAJOR CONTOURS");
                ui.label("(50m)");

                ui.separator();
                ui.colored_label(theme::topo_color(), "MINOR CONTOURS");
                ui.label("terrain grid");

                if model.show_coastlines {
                    ui.separator();
                    ui.colored_label(theme::contour_color(), "COASTLINE");
                    ui.label("outline");
                }

                ui.separator();
                ui.colored_label(theme::road_major_color(), "MAJOR ROADS");
                ui.label("primary network");

                ui.separator();
                ui.colored_label(theme::road_minor_color(), "MINOR ROADS");
                ui.label("secondary network");

                if model.show_water {
                    ui.separator();
                    ui.colored_label(theme::water_color(), "WATER");
                    ui.label("hydrology");
                }

                if local_terrain_scene::is_active(model) {
                    ui.separator();
                    ui.label(format!("Terrain zoom {:.1}x", model.globe_view.local_zoom));
                }

            });
        });
}

fn draw_globe_coord_overlay(ctx: &egui::Context, geo: crate::model::GeoPoint, rect: egui::Rect) {
    let ns = if geo.lat >= 0.0 { 'N' } else { 'S' };
    let ew = if geo.lon >= 0.0 { 'E' } else { 'W' };
    let text = format!("{:.4}°{}  {:.4}°{}", geo.lat.abs(), ns, geo.lon.abs(), ew);
    egui::Area::new("globe_coord_overlay".into())
        .fixed_pos(egui::pos2(rect.center().x - 80.0, rect.bottom() - 28.0))
        .interactable(false)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(theme::panel_fill(200))
                .corner_radius(5.0)
                .inner_margin(egui::Margin::symmetric(8, 4))
                .show(ui, |ui| {
                    ui.colored_label(
                        theme::contour_color(),
                        egui::RichText::new(text).monospace().small(),
                    );
                });
        });
}

fn draw_arcgis_detail_panel(ctx: &egui::Context, model: &mut AppModel) {
    let Some((ref src_url, obj_id)) = model.selected_arcgis_feature.clone() else {
        return;
    };
    let Some(feat) = model
        .arcgis_features
        .iter()
        .find(|f| f.source_url == *src_url && f.object_id == obj_id)
        .cloned()
    else {
        return;
    };

    let col = arcgis_source::feature_color(&feat);

    // Get source/layer name for the title
    let snap = arcgis_source::source_snapshot(&feat.source_url);
    let source_name = snap
        .as_ref()
        .map(|s| s.display_name.as_str())
        .unwrap_or("ArcGIS Feature");
    let layer_name = snap
        .as_ref()
        .and_then(|s| s.layers.as_ref())
        .and_then(|ls| ls.iter().find(|l| l.id == feat.layer_id))
        .map(|l| l.name.as_str())
        .unwrap_or("")
        .to_owned();

    let mut open = true;
    egui::Window::new("Feature Detail")
        .id("arcgis_detail_panel".into())
        .open(&mut open)
        .resizable(true)
        .min_width(280.0)
        .frame(egui::Frame::window(&ctx.style()).stroke(egui::Stroke::new(1.5, col)))
        .show(ctx, |ui| {
            ui.colored_label(col, source_name);
            if !layer_name.is_empty() {
                ui.small(&layer_name);
            }
            ui.separator();

            if let Some(ms) = feat.date_ms {
                ui.horizontal(|ui| {
                    ui.strong("Date:");
                    ui.label(arcgis_source::format_date(ms));
                });
            }

            let ns = if feat.location.lat >= 0.0 { 'N' } else { 'S' };
            let ew = if feat.location.lon >= 0.0 { 'E' } else { 'W' };
            ui.horizontal(|ui| {
                ui.strong("Location:");
                ui.label(
                    egui::RichText::new(format!(
                        "{:.4}°{}  {:.4}°{}",
                        feat.location.lat.abs(),
                        ns,
                        feat.location.lon.abs(),
                        ew,
                    ))
                    .monospace()
                    .small(),
                );
            });

            ui.add_space(4.0);

            // All attributes as key-value pairs
            egui::ScrollArea::vertical()
                .max_height(320.0)
                .show(ui, |ui| {
                    egui::Grid::new("arcgis_attr_grid")
                        .num_columns(2)
                        .striped(true)
                        .show(ui, |ui| {
                            for (key, val) in &feat.attributes {
                                // Skip date fields already shown above
                                if key.eq_ignore_ascii_case("Date")
                                    || key.eq_ignore_ascii_case("date")
                                {
                                    continue;
                                }
                                ui.strong(key);
                                ui.label(val);
                                ui.end_row();
                            }
                        });
                });
        });
    if !open {
        model.selected_arcgis_feature = None;
    }
}
