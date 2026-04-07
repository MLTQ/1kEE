pub(super) mod dissolve;
pub(super) mod geography;
pub(super) mod markers;
pub(super) mod projection;
pub(super) mod ui_overlays;

// Re-export project_local so sibling modules (road_layer, water_layer) can
// import it from local_terrain_scene directly as they did before the split.
pub(super) fn project_local(
    layout: &LocalLayout,
    view: &GlobeViewState,
    focus: GeoPoint,
    point: GeoPoint,
    elevation_m: f32,
    extent_x_km: f32,
    extent_y_km: f32,
) -> Option<ProjectedLocalPoint> {
    projection::project_local(
        layout,
        view,
        focus,
        point,
        elevation_m,
        extent_x_km,
        extent_y_km,
    )
}

use crate::arcgis_source;
use crate::model::{AppModel, ArcGisFeature, GeoPoint, GlobeViewState};
use crate::osm_ingest::{self, GeoBounds as OsmGeoBounds};
use crate::terrain_assets;
use crate::theme;
use std::path::Path;

use super::contour_asset;
use super::globe_scene::GlobeScene;
use super::srtm_focus_cache;
use super::srtm_stream;

#[allow(dead_code)]
pub const LOCAL_TRANSITION_START_ZOOM: f32 = 4.0;
#[allow(dead_code)]
pub const LOCAL_MODE_MIN_ZOOM: f32 = 25.0;
const LOCAL_STREAM_RADIUS: i32 = 2;
pub(super) const BASE_VERTICAL_EXAGGERATION: f32 = 2.1;

// Minimum local zoom value — allows zooming out to ~500 km half-span.
pub const LOCAL_ZOOM_MIN: f32 = 1.0;

#[derive(Clone, Copy)]
pub(super) struct LocalLayout {
    pub(super) center: egui::Pos2,
    pub(super) focus_center: egui::Pos2,
    pub(super) width: f32,
    pub(super) height: f32,
    pub(super) horizontal_scale: f32,
}

#[derive(Clone, Copy)]
pub(super) struct ProjectedLocalPoint {
    pub(super) pos: egui::Pos2,
    pub(super) depth: f32,
}

pub fn paint(painter: &egui::Painter, rect: egui::Rect, model: &AppModel, time: f64) -> GlobeScene {
    painter.rect_filled(rect, 12.0, theme::canvas_background());
    if !model.cinematic_mode {
        dissolve::draw_frame(painter, rect);
    }

    let layout = layout(rect);
    let Some(focus) = model.terrain_focus_location() else {
        ui_overlays::draw_empty_state(painter, rect, "No terrain focus selected");
        return GlobeScene {
            event_markers: Vec::new(),
            camera_markers: Vec::new(),
            ship_markers: Vec::new(),
            flight_markers: Vec::new(),
            arcgis_feature_markers: Vec::new(),
            beam_elevation_m: None,
        };
    };

    let viewport_center = model.globe_view.local_center;
    let render_zoom = local_render_zoom(model.globe_view.local_zoom);

    let contours = if model.moon_mode {
        contour_asset::load_lunar_region_for_view(
            model.selected_root.as_deref(),
            focus,
            viewport_center,
            render_zoom,
            LOCAL_STREAM_RADIUS,
            painter.ctx().clone(),
        )
    } else {
        contour_asset::load_srtm_region_for_view(
            model.selected_root.as_deref(),
            focus,
            viewport_center,
            render_zoom,
            LOCAL_STREAM_RADIUS,
            painter.ctx().clone(),
        )
    };
    let cache_status = if model.moon_mode {
        None // lunar status tracked via is_lunar_contour_building()
    } else {
        srtm_focus_cache::focus_contour_region_status(
            model.selected_root.as_deref(),
            viewport_center,
            render_zoom,
            LOCAL_STREAM_RADIUS,
        )
    };

    let nearby = if model.focused_city().is_none() {
        model.nearby_cameras(250.0)
    } else {
        Vec::new()
    };

    // Pulsing tile-grid glow: only draw cells that are NOT yet ready in the cache.
    let still_loading = if model.moon_mode {
        srtm_focus_cache::is_lunar_contour_building() || contours.is_none()
    } else {
        cache_status
            .map(|s| s.ready_assets < s.total_assets)
            .unwrap_or(contours.is_none())
    };
    if still_loading {
        if model.moon_mode {
            let ready_buckets = srtm_focus_cache::ready_lunar_tile_buckets(
                model.selected_root.as_deref(),
                viewport_center,
                render_zoom,
                LOCAL_STREAM_RADIUS,
            );
            let half_extent = srtm_focus_cache::lunar_half_extent_for_zoom(render_zoom);
            dissolve::draw_tile_pulse_grid(
                painter,
                &layout,
                &model.globe_view,
                viewport_center,
                render_zoom,
                LOCAL_STREAM_RADIUS,
                time,
                &ready_buckets,
                Some(half_extent),
            );
        } else {
            let ready_buckets = srtm_focus_cache::ready_tile_buckets(
                model.selected_root.as_deref(),
                viewport_center,
                render_zoom,
                LOCAL_STREAM_RADIUS,
            );
            dissolve::draw_tile_pulse_grid(
                painter,
                &layout,
                &model.globe_view,
                viewport_center,
                render_zoom,
                LOCAL_STREAM_RADIUS,
                time,
                &ready_buckets,
                None,
            );
        }
    }

    let contours_slice = contours.as_ref().map(|v| v.as_slice()).unwrap_or(&[]);

    // ── Background contour pass (drawn before fill so opaque fill covers them) ─
    if model.fill_elevation && !contours_slice.is_empty() && model.show_contours {
        draw_contour_stack(
            painter,
            &layout,
            &model.globe_view,
            viewport_center,
            render_zoom,
            contours_slice,
            1.0,
            model.moon_mode,
        );
    }

    // ── Elevation fill (opaque — occludes the background contour pass above) ──
    if model.fill_elevation && !contours_slice.is_empty() {
        draw_elevation_fill(
            painter,
            &layout,
            &model.globe_view,
            viewport_center,
            contours_slice,
            model.selected_root.as_deref(),
            model.moon_mode,
        );
    }

    // ── Surface contour pass (drawn after fill so lines on top are visible) ───
    if !contours_slice.is_empty() && model.show_contours {
        draw_contour_stack(
            painter,
            &layout,
            &model.globe_view,
            viewport_center,
            render_zoom,
            contours_slice,
            1.0,
            model.moon_mode,
        );
    }
    // OSM layers (roads, water, trees, buildings) are Earth-only — no data on the Moon.
    if !contours_slice.is_empty() && !model.moon_mode {
        super::road_layer::draw_roads(
            painter,
            &layout,
            &model.globe_view,
            model.selected_root.as_deref(),
            viewport_center,
            render_zoom,
            model.show_major_roads,
            model.show_minor_roads,
        );
        super::water_layer::draw_water(
            painter,
            &layout,
            &model.globe_view,
            model.selected_root.as_deref(),
            viewport_center,
            render_zoom,
            model.show_water,
        );
        super::waterway_layer::draw_waterways(
            painter,
            &layout,
            &model.globe_view,
            model.selected_root.as_deref(),
            viewport_center,
            render_zoom,
            model.show_water,
        );
        super::tree_layer::draw_trees(
            painter,
            &layout,
            &model.globe_view,
            model.selected_root.as_deref(),
            viewport_center,
            render_zoom,
            model.show_trees,
        );
        super::building_layer::draw_buildings(
            painter,
            &layout,
            &model.globe_view,
            model.selected_root.as_deref(),
            viewport_center,
            render_zoom,
            model.show_buildings,
        );
    }

    // ── Admin boundaries (Earth only) ─────────────────────────────────────
    if model.show_admin && !model.moon_mode {
        if let Some(root) = model.selected_root.as_deref() {
            let half_extent_deg = visual_half_extent_for_zoom(model.globe_view.local_zoom);
            let km_per_deg_lat = 111.32f32;
            let km_per_deg_lon =
                km_per_deg_lat * viewport_center.lat.to_radians().cos().abs().max(0.2);
            let extent_x_km = (half_extent_deg * km_per_deg_lon).max(1.0);
            let extent_y_km = (half_extent_deg * km_per_deg_lat).max(1.0);

            // Load once, then render lowest-priority levels first so level 2
            // (country) draws last and sits on top.
            let boundaries = super::admin_layer::get_or_load_admin_boundaries(root, &[2, 4, 6, 8]);

            for &level in &[8u8, 6, 4, 2] {
                let stroke =
                    egui::Stroke::new(theme::admin_stroke_width(level), theme::admin_color(level));
                for boundary in boundaries.iter().filter(|b| b.admin_level == level) {
                    let pts: Vec<egui::Pos2> = boundary
                        .points
                        .iter()
                        .filter_map(|&pt| {
                            projection::project_local(
                                &layout,
                                &model.globe_view,
                                viewport_center,
                                pt,
                                0.0,
                                extent_x_km,
                                extent_y_km,
                            )
                            .map(|p| p.pos)
                        })
                        .collect();
                    if pts.len() >= 2 {
                        painter.add(egui::Shape::line(pts, stroke));
                    }
                }
            }
        }
    }

    // Beam, markers, legend and progress bar always render regardless of load state.
    let beam_elevation_m = draw_local_beam(
        painter,
        rect,
        &layout,
        &model.globe_view,
        viewport_center,
        contours.as_ref().map(|v| v.as_slice()),
        model.show_beam,
    );
    // ── Event beams — all events in the viewport, no contour dependency ───────
    // Mirrors the globe_scene approach: draw every event whose location falls
    // within the visible area, not just the selected one.
    let half_extent_deg = visual_half_extent_for_zoom(model.globe_view.local_zoom);
    let km_per_deg_lat = 111.32f32;
    let km_per_deg_lon = km_per_deg_lat * viewport_center.lat.to_radians().cos().abs().max(0.2);
    let extent_x_km = (half_extent_deg * km_per_deg_lon).max(1.0);
    let extent_y_km = (half_extent_deg * km_per_deg_lat).max(1.0);

    const EVENT_BEAM_HEIGHT_PX: f32 = 110.0;

    let event_markers: Vec<(String, egui::Pos2)> =
        if model.cinematic_mode || !model.show_event_markers {
            Vec::new()
        } else {
            model
                .events
                .iter()
                .filter_map(|event| {
                    // Cheap pre-cull: skip events well outside the viewport.
                    let dlat = (event.location.lat - viewport_center.lat).abs();
                    let dlon = {
                        let d = (event.location.lon - viewport_center.lon).abs();
                        d.min(360.0 - d)
                    };
                    if dlat > half_extent_deg * 2.5 || dlon > half_extent_deg * 2.5 {
                        return None;
                    }
                    let elev =
                        markers::marker_elevation_m(model.selected_root.as_deref(), event.location);
                    let ground = projection::project_local(
                        &layout,
                        &model.globe_view,
                        viewport_center,
                        event.location,
                        elev,
                        extent_x_km,
                        extent_y_km,
                    )?;
                    // Tip: project the same point 1 km higher, then cap the
                    // screen-space length so beams don't vary wildly with tilt.
                    let tip = projection::project_local(
                        &layout,
                        &model.globe_view,
                        viewport_center,
                        event.location,
                        elev + 1000.0,
                        extent_x_km,
                        extent_y_km,
                    )
                    .map(|sky| {
                        let dx = sky.pos.x - ground.pos.x;
                        let dy = sky.pos.y - ground.pos.y;
                        let len = (dx * dx + dy * dy).sqrt().max(0.1);
                        egui::pos2(
                            ground.pos.x + dx / len * EVENT_BEAM_HEIGHT_PX,
                            ground.pos.y + dy / len * EVENT_BEAM_HEIGHT_PX,
                        )
                    })
                    .unwrap_or(egui::pos2(
                        ground.pos.x,
                        ground.pos.y - EVENT_BEAM_HEIGHT_PX,
                    ));
                    markers::draw_event_marker(
                        painter,
                        ground,
                        tip,
                        event,
                        model.selected_event_id.as_deref() == Some(event.id.as_str()),
                        time,
                    );
                    Some((event.id.clone(), ground.pos))
                })
                .collect()
        };

    // Camera markers for all nearby cameras.
    let camera_markers: Vec<(String, egui::Pos2)> = nearby
        .iter()
        .filter_map(|camera| {
            projection::project_local(
                &layout,
                &model.globe_view,
                viewport_center,
                camera.location,
                markers::marker_elevation_m(model.selected_root.as_deref(), camera.location),
                extent_x_km,
                extent_y_km,
            )
            .map(|projected| {
                markers::draw_camera_marker(
                    painter,
                    projected,
                    model.selected_camera_id.as_deref() == Some(camera.id.as_str()),
                );
                (camera.id.clone(), projected.pos)
            })
        })
        .collect();

    // Camera link lines anchor to the selected event if one exists.
    let anchor = event_markers
        .iter()
        .find(|(id, _)| model.selected_event_id.as_deref() == Some(id.as_str()))
        .or_else(|| event_markers.first())
        .map(|(_, pos)| *pos);
    markers::draw_camera_links(painter, anchor, &camera_markers);
    if model.show_coastlines && !model.moon_mode {
        geography::draw_coastlines_local(
            painter,
            &layout,
            &model.globe_view,
            viewport_center,
            render_zoom,
            model.selected_root.as_deref(),
        );
    }
    if model.show_bathymetry && !model.moon_mode {
        geography::draw_bathymetry_local(
            painter,
            &layout,
            &model.globe_view,
            viewport_center,
            render_zoom,
            model.selected_root.as_deref(),
        );
    }
    if !model.geojson_layers.is_empty() {
        draw_geojson_layers_local(
            painter,
            &layout,
            &model.globe_view,
            model.selected_root.as_deref(),
            viewport_center,
            extent_x_km,
            extent_y_km,
            &model.geojson_layers,
        );
    }
    ui_overlays::draw_legend(
        painter,
        rect,
        if model.moon_mode {
            "LOCAL LUNAR TERRAIN"
        } else {
            "LOCAL EVENT TERRAIN"
        },
        render_zoom,
        model.moon_mode,
    );
    let (lunar_ready, lunar_building, lunar_total) = if model.moon_mode {
        srtm_focus_cache::lunar_tile_counts(
            model.selected_root.as_deref(),
            viewport_center,
            render_zoom,
            LOCAL_STREAM_RADIUS,
        )
    } else {
        (0, 0, 0)
    };
    ui_overlays::draw_progress_overlay(
        painter,
        rect,
        cache_status,
        osm_ingest::osmium_cell_progress(),
        osm_ingest::active_job_note().as_deref(),
        lunar_building,
        lunar_ready,
        lunar_total,
    );

    let arcgis_feature_markers = if !model.arcgis_features.is_empty() {
        draw_arcgis_features_local(
            painter,
            &layout,
            &model.globe_view,
            model.selected_root.as_deref(),
            viewport_center,
            extent_x_km,
            extent_y_km,
            &model.arcgis_features,
            model.selected_arcgis_feature.as_ref(),
        )
    } else {
        Vec::new()
    };

    GlobeScene {
        event_markers,
        camera_markers,
        ship_markers: Vec::new(),
        flight_markers: Vec::new(),
        arcgis_feature_markers,
        beam_elevation_m: Some(beam_elevation_m),
    }
}

#[allow(dead_code)]
pub fn paint_transition_overlay(
    painter: &egui::Painter,
    rect: egui::Rect,
    model: &AppModel,
    progress: f32,
) {
    if progress <= 0.0 {
        return;
    }

    let Some(focus) = model.terrain_focus_location() else {
        return;
    };

    let viewport_center = model.globe_view.local_center;
    let render_zoom = local_render_zoom(model.globe_view.local_zoom);
    let Some(contours) = contour_asset::load_srtm_region_for_view(
        model.selected_root.as_deref(),
        focus,
        viewport_center,
        render_zoom,
        LOCAL_STREAM_RADIUS,
        painter.ctx().clone(),
    ) else {
        return;
    };

    let layout = transition_layout(rect, progress);
    draw_contour_stack(
        painter,
        &layout,
        &model.globe_view,
        viewport_center,
        render_zoom,
        contours.as_ref(),
        progress,
        model.moon_mode,
    );
}

pub fn is_active(model: &AppModel) -> bool {
    if !model.globe_view.local_mode || model.terrain_focus_location().is_none() {
        return false;
    }
    if model.moon_mode {
        // Lunar local mode: requires SLDEM2015 JP2 source.
        terrain_assets::find_sldem_jp2(model.selected_root.as_deref()).is_some()
    } else {
        terrain_assets::find_srtm_root(model.selected_root.as_deref()).is_some()
    }
}

#[allow(dead_code)]
pub fn transition_progress(zoom: f32) -> f32 {
    ((zoom - LOCAL_TRANSITION_START_ZOOM) / (LOCAL_MODE_MIN_ZOOM - LOCAL_TRANSITION_START_ZOOM))
        .clamp(0.0, 1.0)
}

pub fn has_pending_cache(model: &AppModel) -> bool {
    let Some(_) = model.terrain_focus_location() else {
        return false;
    };

    if model.moon_mode {
        return srtm_focus_cache::is_lunar_contour_building();
    }

    let render_zoom = local_render_zoom(model.globe_view.local_zoom);
    srtm_focus_cache::focus_contour_region_status(
        model.selected_root.as_deref(),
        model.globe_view.local_center,
        render_zoom,
        LOCAL_STREAM_RADIUS,
    )
    .map(|status| status.ready_assets < status.total_assets)
    .unwrap_or(true)
}

pub fn local_render_zoom(local_zoom: f32) -> f32 {
    // local_zoom lives in [LOCAL_ZOOM_MIN, 60].
    // Tile-spec resolution is capped at 20 (finest bucket); above 20 only
    // the visual scale continues to change.  Below ~4 the coarsest bucket
    // (zoom_bucket=0, half_extent=3.6°) handles the wide-area view.
    local_zoom.clamp(LOCAL_ZOOM_MIN, 20.0)
}

pub fn visual_half_extent_for_zoom(view_zoom: f32) -> f32 {
    // Continuous logarithmic progression from widest (~500 km) to narrowest (~0.6 km).
    // local_zoom ∈ [1, 20] also shifts the tile-spec bucket; above 20 only
    // the visual scale changes (finest tiles stay loaded).
    const KNOTS: &[(f32, f32)] = &[
        (1.0, 4.50),   // ~500 km
        (2.0, 2.80),   // ~311 km
        (3.0, 1.95),   // ~217 km
        (4.0, 1.55),   // ~173 km
        (5.5, 0.90),   // ~100 km
        (7.0, 0.55),   // ~61 km
        (9.5, 0.31),   // ~35 km
        (12.0, 0.17),  // ~19 km
        (16.0, 0.09),  // ~10 km
        (20.0, 0.050), // ~5.5 km
        (28.0, 0.025), // ~2.8 km
        (40.0, 0.012), // ~1.3 km
        (60.0, 0.005), // ~0.6 km
    ];

    let zoom = view_zoom.clamp(LOCAL_ZOOM_MIN, 60.0);
    for window in KNOTS.windows(2) {
        let (start_zoom, start_extent) = window[0];
        let (end_zoom, end_extent) = window[1];
        if zoom <= end_zoom {
            let t = ((zoom - start_zoom) / (end_zoom - start_zoom)).clamp(0.0, 1.0);
            let start_log = start_extent.ln();
            let end_log = end_extent.ln();
            return egui::lerp(start_log..=end_log, t).exp();
        }
    }

    KNOTS.last().map(|(_, extent)| *extent).unwrap_or(0.17)
}

fn layout(rect: egui::Rect) -> LocalLayout {
    let width = rect.width() * 0.82;
    let height = rect.height() * 0.74;
    LocalLayout {
        center: rect.center(),
        focus_center: egui::pos2(
            rect.center().x + rect.width() * 0.02,
            rect.center().y + 12.0,
        ),
        width,
        height,
        horizontal_scale: rect.width() * 0.31,
    }
}

#[allow(dead_code)]
fn transition_layout(rect: egui::Rect, progress: f32) -> LocalLayout {
    let progress = progress.clamp(0.0, 1.0);
    let target = layout(rect);
    let scale = egui::lerp(0.52..=1.0, progress);
    let vertical_origin = egui::lerp(
        (rect.center().y + rect.height() * 0.1)..=(target.focus_center.y),
        progress,
    );

    LocalLayout {
        center: target.center,
        focus_center: egui::pos2(target.focus_center.x, vertical_origin),
        width: target.width * scale,
        height: target.height * scale,
        horizontal_scale: target.horizontal_scale * scale,
    }
}

/// Cherry-red targeting beam: a vertical line falling from the sky to the
/// terrain surface at the viewport centre. The ground contact point is
/// projected via `project_local` so it rises over hills and drops into
/// valleys as the map is dragged beneath the fixed beam.
/// Always returns the computed terrain elevation (metres) even when `show` is false.
fn draw_local_beam(
    painter: &egui::Painter,
    rect: egui::Rect,
    layout: &LocalLayout,
    view: &GlobeViewState,
    viewport_center: GeoPoint,
    contours: Option<&[contour_asset::ContourPath]>,
    show: bool,
) -> f32 {
    let cherry = egui::Color32::from_rgb(210, 18, 50);

    // Derive terrain elevation at the crosshair from the loaded contour data —
    // the same data used to draw the terrain, so it's always available and in sync.
    // Find the highest-elevation contour that has a point within a tight radius
    // of viewport_center; that contour passes through (or very near) center,
    // so its elevation approximates the terrain surface there.
    let half_extent_deg = visual_half_extent_for_zoom(view.local_zoom);
    let search_radius_deg = (half_extent_deg * 0.08).max(0.004); // ~8% of viewport radius
    let elevation_m = contours
        .unwrap_or(&[])
        .iter()
        .filter(|c| {
            c.points.iter().any(|p| {
                (p.lat - viewport_center.lat).abs() < search_radius_deg
                    && (p.lon - viewport_center.lon).abs() < search_radius_deg
            })
        })
        .map(|c| c.elevation_m)
        .fold(0.0f32, f32::max);

    if !show {
        return elevation_m;
    }

    let half_extent_deg = visual_half_extent_for_zoom(view.local_zoom);
    let km_per_deg_lat = 111.32f32;
    let km_per_deg_lon = km_per_deg_lat * viewport_center.lat.to_radians().cos().abs().max(0.2);
    let extent_x_km = (half_extent_deg * km_per_deg_lon).max(1.0);
    let extent_y_km = (half_extent_deg * km_per_deg_lat).max(1.0);

    // Project the centre point at actual terrain elevation — x will always
    // land at focus_center.x (centre of screen) since lat/lon offset is zero.
    let ground = projection::project_local(
        layout,
        view,
        viewport_center,
        viewport_center,
        elevation_m,
        extent_x_km,
        extent_y_km,
    )
    .map(|p| p.pos)
    .unwrap_or(layout.focus_center);
    let sky_top = egui::pos2(ground.x, rect.top() + 14.0);
    // Mid-point: beam is more transparent higher up, brightens as it approaches ground
    let mid = egui::pos2(ground.x, egui::lerp(sky_top.y..=ground.y, 0.45));

    // Wide outer glow — faint, covers full height for soft atmospheric halo
    painter.line_segment(
        [sky_top, ground],
        egui::Stroke::new(8.0, cherry.gamma_multiply(0.04)),
    );

    // Mid glow — starts from halfway down so the lower beam is brighter
    painter.line_segment(
        [mid, ground],
        egui::Stroke::new(4.5, cherry.gamma_multiply(0.13)),
    );

    // Crisp beam — full height, low alpha at top to full alpha at bottom
    // Approximated by layering: faint full-height + bright lower two-thirds
    let lower = egui::pos2(ground.x, egui::lerp(sky_top.y..=ground.y, 0.28));
    painter.line_segment(
        [sky_top, ground],
        egui::Stroke::new(1.1, cherry.gamma_multiply(0.30)),
    );
    painter.line_segment(
        [lower, ground],
        egui::Stroke::new(1.1, cherry.gamma_multiply(0.62)),
    );

    // Ground-strike: small horizontal tick where the beam hits the terrain
    let tick = 9.0;
    painter.line_segment(
        [
            egui::pos2(ground.x - tick, ground.y),
            egui::pos2(ground.x + tick, ground.y),
        ],
        egui::Stroke::new(1.3, cherry.gamma_multiply(0.90)),
    );

    // Glow halo at the contact point
    painter.circle_stroke(
        ground,
        6.5,
        egui::Stroke::new(4.0, cherry.gamma_multiply(0.10)),
    );
    painter.circle_stroke(
        ground,
        5.0,
        egui::Stroke::new(1.2, cherry.gamma_multiply(0.78)),
    );
    painter.circle_filled(ground, 1.8, cherry);

    elevation_m
}

// ── Elevation fill (hypsometric tint + hillshade) ─────────────────────────────

type ElevFillKey = (i32, i32, i32, i32, i32, i32, i32, i32, u8);

struct ElevFillEntry {
    key: ElevFillKey,
    mesh: egui::Mesh,
}

struct ElevFillState {
    /// Key for which a background build is in-flight.
    building_key: Option<ElevFillKey>,
    /// Channel from the background thread.
    result_rx: Option<std::sync::mpsc::Receiver<(ElevFillKey, egui::Mesh)>>,
    /// Last successfully built mesh (may be stale while a new one is building).
    ready: Option<ElevFillEntry>,
}

static ELEV_FILL: std::sync::OnceLock<std::sync::Mutex<ElevFillState>> = std::sync::OnceLock::new();

fn elev_fill_key(
    focus: GeoPoint,
    view: &GlobeViewState,
    layout: &LocalLayout,
    contour_count: usize,
    gebco_sample_count: usize,
) -> ElevFillKey {
    (
        (focus.lat * 100.0) as i32,
        (focus.lon * 100.0) as i32,
        (view.local_zoom * 10.0) as i32,
        (view.local_yaw * 100.0) as i32,
        (view.local_pitch * 100.0) as i32,
        (view.local_layer_spread * 100.0) as i32,
        // Contour count changes as background threads finish loading tiles.
        // Layout scale changes on window resize.  Both must invalidate the mesh.
        contour_count as i32 ^ (layout.horizontal_scale * 0.5) as i32,
        // GEBCO sample count: invalidates when bathymetry data finishes loading.
        gebco_sample_count as i32,
        theme::hot_color().r(), // proxy for theme identity
    )
}

fn elev_lerp(a: egui::Color32, b: egui::Color32, t: f32) -> egui::Color32 {
    let t = t.clamp(0.0, 1.0);
    egui::Color32::from_rgb(
        (a.r() as f32 + (b.r() as f32 - a.r() as f32) * t) as u8,
        (a.g() as f32 + (b.g() as f32 - a.g() as f32) * t) as u8,
        (a.b() as f32 + (b.b() as f32 - a.b() as f32) * t) as u8,
    )
}

fn elevation_fill_color_lunar(elev_m: f32) -> egui::Color32 {
    // Monochrome regolith palette: dark mare basalt → mid highland → sunlit peak.
    // Moon elevation range here: ~-9000 m (SPA basin) to +11000 m (highland rims).
    let mare = egui::Color32::from_rgb(28, 27, 32); // dark mare basalt
    let lowland = egui::Color32::from_rgb(60, 58, 68); // low highland
    let mid = egui::Color32::from_rgb(110, 108, 118); // mid highland (most surface)
    let high = egui::Color32::from_rgb(168, 165, 152); // high terrain
    let peak = egui::Color32::from_rgb(215, 210, 188); // sunlit peaks / rims

    if elev_m < -2000.0 {
        elev_lerp(lowland, mare, ((-elev_m - 2000.0) / 6000.0).min(1.0))
    } else if elev_m < 0.0 {
        elev_lerp(mid, lowland, -elev_m / 2000.0)
    } else if elev_m < 3000.0 {
        elev_lerp(mid, high, elev_m / 3000.0)
    } else if elev_m < 7000.0 {
        elev_lerp(high, peak, (elev_m - 3000.0) / 4000.0)
    } else {
        peak
    }
}

fn elevation_fill_color(elev_m: f32) -> egui::Color32 {
    // Use theme colors that are clearly distinguishable from the dark canvas background.
    // canvas_background() ≈ rgb(18,44,56) — very dark.
    // topo_color()        ≈ rgb(39,88,105) — only slightly lighter → looks invisible.
    // contour_color()     ≈ rgb(96,164,181) — noticeably lighter → clearly visible.
    // hot_color()         ≈ rgb(245,125,78) — warm/bright → peaks.
    let ocean = theme::canvas_background(); // deep water
    let shore = theme::topo_color(); // shallow / coastline
    let land = theme::contour_color(); // main land surface — must contrast with bg
    let peak = theme::hot_color(); // mountain peaks

    if elev_m < -500.0 {
        elev_lerp(shore, ocean, ((-elev_m - 500.0) / 3000.0).min(1.0))
    } else if elev_m < 0.0 {
        elev_lerp(land, shore, (-elev_m / 500.0))
    } else if elev_m < 600.0 {
        elev_lerp(land, elev_lerp(land, peak, 0.4), elev_m / 600.0)
    } else if elev_m < 2500.0 {
        elev_lerp(elev_lerp(land, peak, 0.4), peak, (elev_m - 600.0) / 1900.0)
    } else {
        peak
    }
}

fn build_elev_fill_mesh(
    layout: &LocalLayout,
    view: &GlobeViewState,
    focus: GeoPoint,
    contours: &[contour_asset::ContourPath],
    gebco_samples: &[(f32, f32, f32)],
    moon_mode: bool,
) -> egui::Mesh {
    const N: usize = 60; // 61×61 = 3,721 vertices, 7,200 triangles

    let half_extent_deg = visual_half_extent_for_zoom(view.local_zoom);
    let km_per_deg_lat = 111.32f32;
    let km_per_deg_lon = km_per_deg_lat * focus.lat.to_radians().cos().abs().max(0.2);
    let extent_x_km = (half_extent_deg * km_per_deg_lon).max(1.0);
    let extent_y_km = (half_extent_deg * km_per_deg_lat).max(1.0);
    let cell_size_m = (2.0 * half_extent_deg * 111_320.0 / N as f32).max(1.0);

    // Build elevation surface from the contour data already loaded.
    // Strategy: take up to MAX_SAMPLES representative (lat, lon, elevation_m) points
    // from the contour polylines, then use inverse-distance-weighted (IDW) interpolation
    // to estimate elevation at each fill-mesh vertex.
    //
    // GEBCO bathymetry midpoints are merged in so that ocean areas receive proper
    // negative elevation estimates (dark blue) rather than extrapolated land values.
    const MAX_SAMPLES: usize = 400;
    let stride = (contours.len() / MAX_SAMPLES).max(1);
    let mut samples: Vec<(f32, f32, f32)> = contours
        .iter()
        .step_by(stride)
        .filter_map(|c| {
            if c.points.is_empty() {
                return None;
            }
            // Use the midpoint of each contour arc (more representative than centroid
            // for long arcs that curve around topographic features).
            let mid = &c.points[c.points.len() / 2];
            Some((mid.lat, mid.lon, c.elevation_m))
        })
        .collect();

    // Append GEBCO ocean-floor samples (already filtered to viewport by caller).
    samples.extend_from_slice(gebco_samples);

    // IDW elevation estimate for a (lat, lon) given the sample set.
    // Power = 2 (inverse square distance).  Sea level (0.0) if no samples.
    let idw_elevation = |lat: f32, lon: f32| -> f32 {
        if samples.is_empty() {
            return 0.0;
        }
        let mut wsum = 0.0f32;
        let mut esum = 0.0f32;
        for &(slat, slon, elev) in &samples {
            let dlat = lat - slat;
            let dlon = lon - slon;
            let dist2 = dlat * dlat + dlon * dlon;
            if dist2 < 1e-10 {
                return elev; // exactly on a sample point
            }
            let w = 1.0 / dist2;
            wsum += w;
            esum += w * elev;
        }
        esum / wsum
    };

    // Sample elevations into a flat grid so we can compute normals
    let side = N + 1;
    let mut elevs = vec![0.0f32; side * side];
    for row in 0..side {
        for col in 0..side {
            let lat =
                (focus.lat - half_extent_deg) + (row as f32 / N as f32) * 2.0 * half_extent_deg;
            let lon =
                (focus.lon - half_extent_deg) + (col as f32 / N as f32) * 2.0 * half_extent_deg;
            elevs[row * side + col] = idw_elevation(lat, lon);
        }
    }

    let mut mesh = egui::Mesh::default();
    let mut vertex_depths: Vec<f32> = Vec::with_capacity(side * side);
    mesh.vertices.reserve(side * side);
    mesh.indices.reserve(N * N * 6);

    for row in 0..side {
        for col in 0..side {
            let elev = elevs[row * side + col];

            // Central-difference normals for hillshade (world-space, Z-up)
            let dzdx = if col > 0 && col < N {
                (elevs[row * side + col + 1] - elevs[row * side + col - 1]) / (2.0 * cell_size_m)
            } else if col == 0 {
                (elevs[row * side + 1] - elev) / cell_size_m
            } else {
                (elev - elevs[row * side + col - 1]) / cell_size_m
            };
            let dzdy = if row > 0 && row < N {
                (elevs[(row - 1) * side + col] - elevs[(row + 1) * side + col])
                    / (2.0 * cell_size_m)
            } else if row == 0 {
                (elev - elevs[side + col]) / cell_size_m
            } else {
                (elevs[(row - 1) * side + col] - elev) / cell_size_m
            };
            let len = (dzdx * dzdx + dzdy * dzdy + 1.0).sqrt();
            let nx = -dzdx / len;
            let ny = dzdy / len;
            let nz = 1.0 / len;

            // Sun from upper-right
            let (lx, ly, lz) = (0.5f32, 0.8, 1.2);
            let llen = (lx * lx + ly * ly + lz * lz).sqrt();
            // Ambient=0.6 so shadows never darken below 60% — keeps land color
            // clearly visible against the near-black canvas background.
            let shade = 0.60 + 0.40 * (nx * lx / llen + ny * ly / llen + nz * lz / llen).max(0.0);

            let base = if moon_mode {
                elevation_fill_color_lunar(elev)
            } else {
                elevation_fill_color(elev)
            };
            // Apply hillshade to RGB only — gamma_multiply would also reduce alpha,
            // making the mesh semi-transparent.  Keep alpha=255 (fully opaque).
            let color = egui::Color32::from_rgb(
                (base.r() as f32 * shade).min(255.0) as u8,
                (base.g() as f32 * shade).min(255.0) as u8,
                (base.b() as f32 * shade).min(255.0) as u8,
            );

            let lat =
                (focus.lat - half_extent_deg) + (row as f32 / N as f32) * 2.0 * half_extent_deg;
            let lon =
                (focus.lon - half_extent_deg) + (col as f32 / N as f32) * 2.0 * half_extent_deg;
            let projected = projection::project_local(
                layout,
                view,
                focus,
                GeoPoint { lat, lon },
                elev,
                extent_x_km,
                extent_y_km,
            );
            let (pos, depth) = projected.map(|p| (p.pos, p.depth)).unwrap_or_else(|| {
                (
                    egui::pos2(
                        layout.focus_center.x
                            + (col as f32 / N as f32 - 0.5) * layout.horizontal_scale * 2.0,
                        layout.focus_center.y - (row as f32 / N as f32 - 0.5) * layout.height,
                    ),
                    0.5,
                )
            });

            vertex_depths.push(depth);
            mesh.vertices.push(egui::epaint::Vertex {
                pos,
                uv: egui::pos2(0.0, 0.0),
                color,
            });
        }
    }

    // Sort triangles back-to-front (painter's algorithm) so nearer terrain
    // correctly occludes terrain behind it without a z-buffer.
    let mut tris: Vec<[u32; 3]> = Vec::with_capacity(N * N * 2);
    for row in 0..N {
        for col in 0..N {
            let v = |r: usize, c: usize| (r * side + c) as u32;
            tris.push([v(row, col), v(row + 1, col), v(row, col + 1)]);
            tris.push([v(row + 1, col), v(row + 1, col + 1), v(row, col + 1)]);
        }
    }
    tris.sort_unstable_by(|a, b| {
        let da = (vertex_depths[a[0] as usize]
            + vertex_depths[a[1] as usize]
            + vertex_depths[a[2] as usize])
            / 3.0;
        let db = (vertex_depths[b[0] as usize]
            + vertex_depths[b[1] as usize]
            + vertex_depths[b[2] as usize])
            / 3.0;
        // Descending: far triangles (high depth) first so near ones paint over them
        db.partial_cmp(&da).unwrap_or(std::cmp::Ordering::Equal)
    });
    for tri in &tris {
        mesh.indices.extend_from_slice(tri);
    }

    mesh
}

fn draw_elevation_fill(
    painter: &egui::Painter,
    layout: &LocalLayout,
    view: &GlobeViewState,
    focus: GeoPoint,
    contours: &[contour_asset::ContourPath],
    selected_root: Option<&std::path::Path>,
    moon_mode: bool,
) {
    // Load GEBCO bathymetry contours and extract midpoints within the viewport.
    // These provide ocean-floor elevation samples so IDW gives negative elevations
    // for ocean areas instead of extrapolating from land contours.
    let half_extent_deg = visual_half_extent_for_zoom(view.local_zoom);
    let margin = half_extent_deg * 2.0;
    let min_lat = focus.lat - margin;
    let max_lat = focus.lat + margin;
    let min_lon = focus.lon - margin;
    let max_lon = focus.lon + margin;

    let bathy_zoom = view.local_zoom.clamp(1.0, 8.0);
    // Moon has no oceans — skip GEBCO bathymetry samples entirely.
    let gebco_samples: Vec<(f32, f32, f32)> = if moon_mode {
        Vec::new()
    } else if let Some(bathy) =
        contour_asset::load_global_bathymetry(selected_root, bathy_zoom, painter.ctx().clone())
    {
        bathy
            .iter()
            .filter(|c| {
                c.points.iter().any(|p| {
                    p.lat >= min_lat && p.lat <= max_lat && p.lon >= min_lon && p.lon <= max_lon
                })
            })
            .filter_map(|c| {
                // Pick the midpoint of each GEBCO arc that falls within viewport.
                let mid_candidates: Vec<_> = c
                    .points
                    .iter()
                    .filter(|p| {
                        p.lat >= min_lat && p.lat <= max_lat && p.lon >= min_lon && p.lon <= max_lon
                    })
                    .collect();
                if mid_candidates.is_empty() {
                    return None;
                }
                let mid = mid_candidates[mid_candidates.len() / 2];
                Some((mid.lat, mid.lon, c.elevation_m))
            })
            .collect()
    } else {
        Vec::new()
    };

    let key = elev_fill_key(
        focus,
        view,
        layout,
        contours.len(),
        gebco_samples.len() + if moon_mode { 100_000 } else { 0 },
    );
    let state_mutex = ELEV_FILL.get_or_init(|| {
        std::sync::Mutex::new(ElevFillState {
            building_key: None,
            result_rx: None,
            ready: None,
        })
    });
    let mut state = state_mutex.lock().unwrap();

    // Poll for a completed background build (non-blocking).
    let got_result = if let Some(rx) = &state.result_rx {
        match rx.try_recv() {
            Ok((built_key, mesh)) => Some((built_key, mesh)),
            Err(_) => None,
        }
    } else {
        None
    };
    if let Some((built_key, mesh)) = got_result {
        state.ready = Some(ElevFillEntry {
            key: built_key,
            mesh,
        });
        state.building_key = None;
        state.result_rx = None;
        painter.ctx().request_repaint();
    }

    // Kick off a background build when the key has changed and no build is in-flight.
    let need_build = state.ready.as_ref().map(|e| e.key != key).unwrap_or(true);
    if need_build && state.building_key != Some(key) {
        let layout_c = *layout;
        let view_c = *view;
        let contours_c: Vec<_> = contours.to_vec();
        let gebco_c = gebco_samples;
        let ctx = painter.ctx().clone();
        let (tx, rx) = std::sync::mpsc::channel();
        state.building_key = Some(key);
        state.result_rx = Some(rx);
        std::thread::spawn(move || {
            let mesh =
                build_elev_fill_mesh(&layout_c, &view_c, focus, &contours_c, &gebco_c, moon_mode);
            let _ = tx.send((key, mesh));
            ctx.request_repaint();
        });
    }

    // Render the last ready mesh (stale is fine while a new one is building).
    if let Some(entry) = &state.ready {
        painter.add(egui::Shape::mesh(entry.mesh.clone()));
    }
}

fn draw_contour_stack(
    painter: &egui::Painter,
    layout: &LocalLayout,
    view: &GlobeViewState,
    focus: GeoPoint,
    _render_zoom: f32,
    contours: &[contour_asset::ContourPath],
    alpha: f32,
    moon_mode: bool,
) {
    // Major contour every 2× the minor interval. SRTM minor=5-50m so major at 50m rem.
    // Lunar minor=50-1000m so major at 1000m rem (two minor intervals up in any spec).
    let major_rem: i32 = if moon_mode { 1_000 } else { 50 };
    let half_extent_deg = visual_half_extent_for_zoom(view.local_zoom);
    let km_per_deg_lat = 111.32f32;
    let km_per_deg_lon = km_per_deg_lat * focus.lat.to_radians().cos().abs().max(0.2);
    let extent_x_km = (half_extent_deg * km_per_deg_lon).max(1.0);
    let extent_y_km = (half_extent_deg * km_per_deg_lat).max(1.0);

    let mut ordered: Vec<_> = contours.iter().collect();
    ordered.sort_by(|left, right| left.elevation_m.total_cmp(&right.elevation_m));

    for contour in ordered {
        let points: Vec<_> = contour
            .points
            .iter()
            .filter_map(|point| {
                projection::project_local(
                    layout,
                    view,
                    focus,
                    *point,
                    contour.elevation_m,
                    extent_x_km,
                    extent_y_km,
                )
                .map(|projected| projected.pos)
            })
            .collect();

        if points.len() < 2 {
            continue;
        }

        let major = (contour.elevation_m.round() as i32).rem_euclid(major_rem) == 0;
        let stroke = egui::Stroke::new(
            if major { 1.35 } else { 0.7 } * (0.72 + alpha * 0.28),
            if major {
                theme::hot_color()
            } else {
                theme::contour_color()
            }
            .gamma_multiply((if major { 1.0 } else { 0.78 }) * alpha),
        );

        painter.add(egui::Shape::line(points, stroke));
    }
}

// ── Road / Water cache public API ─────────────────────────────────────────
// The implementations live in the sibling modules road_layer and water_layer.

/// Clear the road tile cache so the next draw reloads from SQLite.
pub fn invalidate_road_cache() {
    super::road_layer::invalidate_road_cache();
}

/// True while a background road-cache build is in progress.
pub fn road_cache_building() -> bool {
    super::road_layer::road_cache_building()
}

/// Clear the water tile cache so the next draw reloads from SQLite.
pub fn invalidate_water_cache() {
    super::water_layer::invalidate_water_cache();
}

/// True while a background water-cache build is in progress.
pub fn water_cache_building() -> bool {
    super::water_layer::water_cache_building()
}

/// Draw all visible GeoJSON layers in the local oblique terrain view.
fn draw_geojson_layers_local(
    painter: &egui::Painter,
    layout: &LocalLayout,
    view: &GlobeViewState,
    selected_root: Option<&Path>,
    focus: GeoPoint,
    extent_x_km: f32,
    extent_y_km: f32,
    layers: &[crate::model::GeoJsonLayer],
) {
    use crate::model::{GeoJsonGeometry, ring_centroid};

    // Coarse viewport cull bounds (generous margin for lines crossing the border)
    let half_deg_lat = extent_y_km / 111.32 * 1.5;
    let half_deg_lon = (extent_x_km / (111.32 * focus.lat.to_radians().cos().abs().max(0.2))) * 1.5;
    let min_lat = focus.lat - half_deg_lat;
    let max_lat = focus.lat + half_deg_lat;
    let min_lon = focus.lon - half_deg_lon;
    let max_lon = focus.lon + half_deg_lon;

    let in_view_pt =
        |p: &GeoPoint| p.lat >= min_lat && p.lat <= max_lat && p.lon >= min_lon && p.lon <= max_lon;

    for layer in layers {
        if !layer.visible {
            continue;
        }
        let [r, g, b, a] = layer.color;
        let color = egui::Color32::from_rgba_unmultiplied(r, g, b, a);
        let stroke = egui::Stroke::new(1.5, color);

        for feature in &layer.features {
            // ── Draw geometry ─────────────────────────────────────────────
            match &feature.geometry {
                GeoJsonGeometry::Point(pt) => {
                    if in_view_pt(pt) {
                        let elev = srtm_stream::sample_elevation_m(selected_root, *pt)
                            .unwrap_or(0.0)
                            + 18.0;
                        if let Some(pp) = projection::project_local(
                            layout,
                            view,
                            focus,
                            *pt,
                            elev,
                            extent_x_km,
                            extent_y_km,
                        ) {
                            painter.circle_filled(pp.pos, 4.0, color);
                            painter.circle_stroke(
                                pp.pos,
                                6.5,
                                egui::Stroke::new(1.0, color.gamma_multiply(0.4)),
                            );
                        }
                    }
                }
                GeoJsonGeometry::LineString(pts) => {
                    project_and_draw_line(
                        painter,
                        layout,
                        view,
                        selected_root,
                        focus,
                        pts,
                        extent_x_km,
                        extent_y_km,
                        stroke,
                    );
                }
                GeoJsonGeometry::MultiLineString(lines) => {
                    for line in lines {
                        project_and_draw_line(
                            painter,
                            layout,
                            view,
                            selected_root,
                            focus,
                            line,
                            extent_x_km,
                            extent_y_km,
                            stroke,
                        );
                    }
                }
                GeoJsonGeometry::Polygon(rings) => {
                    for ring in rings {
                        project_and_draw_line(
                            painter,
                            layout,
                            view,
                            selected_root,
                            focus,
                            ring,
                            extent_x_km,
                            extent_y_km,
                            stroke,
                        );
                    }
                }
                GeoJsonGeometry::MultiPolygon(polys) => {
                    for poly in polys {
                        for ring in poly {
                            project_and_draw_line(
                                painter,
                                layout,
                                view,
                                selected_root,
                                focus,
                                ring,
                                extent_x_km,
                                extent_y_km,
                                stroke,
                            );
                        }
                    }
                }
            }

            // ── Draw label ────────────────────────────────────────────────
            let Some(label) = &feature.label else {
                continue;
            };
            let anchor = match &feature.geometry {
                GeoJsonGeometry::Point(pt) => Some(*pt),
                GeoJsonGeometry::LineString(pts) if !pts.is_empty() => Some(pts[pts.len() / 2]),
                GeoJsonGeometry::MultiLineString(lines)
                    if !lines.is_empty() && !lines[0].is_empty() =>
                {
                    Some(lines[0][lines[0].len() / 2])
                }
                GeoJsonGeometry::Polygon(rings) if !rings.is_empty() => ring_centroid(&rings[0]),
                GeoJsonGeometry::MultiPolygon(polys)
                    if !polys.is_empty() && !polys[0].is_empty() =>
                {
                    ring_centroid(&polys[0][0])
                }
                _ => None,
            };
            if let Some(pt) = anchor {
                if in_view_pt(&pt) {
                    let label_elev = srtm_stream::sample_elevation_m(selected_root, pt)
                        .unwrap_or(0.0)
                        + 18.0;
                    if let Some(pp) = projection::project_local(
                        layout,
                        view,
                        focus,
                        pt,
                        label_elev,
                        extent_x_km,
                        extent_y_km,
                    ) {
                        painter.text(
                            egui::pos2(pp.pos.x, pp.pos.y - 9.0),
                            egui::Align2::CENTER_BOTTOM,
                            label,
                            egui::FontId::proportional(9.5),
                            color,
                        );
                    }
                }
            }
        }
    }
}

/// Draw ArcGIS features as elevated dot markers in the local terrain scene.
/// Returns (source_url, object_id, screen_pos) for click detection.
fn draw_arcgis_features_local(
    painter: &egui::Painter,
    layout: &LocalLayout,
    view: &GlobeViewState,
    selected_root: Option<&Path>,
    focus: GeoPoint,
    extent_x_km: f32,
    extent_y_km: f32,
    features: &[ArcGisFeature],
    selected: Option<&(String, i64)>,
) -> Vec<(String, i64, egui::Pos2)> {
    let mut markers_out = Vec::new();
    for feat in features {
        let elev =
            srtm_stream::sample_elevation_m(selected_root, feat.location).unwrap_or(0.0) + 18.0;
        let Some(pp) = projection::project_local(
            layout,
            view,
            focus,
            feat.location,
            elev,
            extent_x_km,
            extent_y_km,
        ) else {
            continue;
        };

        let col = arcgis_source::feature_color(feat);
        let pos = pp.pos;
        let has_cas = feat.has_casualties();

        let is_selected = selected
            .map(|(u, id)| u == &feat.source_url && *id == feat.object_id)
            .unwrap_or(false);
        if is_selected {
            painter.circle_stroke(
                pos,
                if has_cas { 13.0 } else { 11.0 },
                egui::Stroke::new(1.5, egui::Color32::WHITE),
            );
        }

        painter.circle_stroke(
            pos,
            if has_cas { 9.0 } else { 6.5 },
            egui::Stroke::new(2.5, col.gamma_multiply(0.12)),
        );
        if has_cas {
            painter.circle_stroke(pos, 6.5, egui::Stroke::new(1.5, col.gamma_multiply(0.22)));
        }

        painter.circle_filled(pos, if has_cas { 3.5 } else { 2.5 }, col);
        painter.circle_filled(pos, 1.2, col.gamma_multiply(1.4));

        markers_out.push((feat.source_url.clone(), feat.object_id, pos));
    }
    markers_out
}

/// Project a polyline into local-terrain screen space and add a line shape.
/// Each vertex is elevated to the terrain surface so lines hug the topology.
fn project_and_draw_line(
    painter: &egui::Painter,
    layout: &LocalLayout,
    view: &GlobeViewState,
    selected_root: Option<&Path>,
    focus: GeoPoint,
    pts: &[GeoPoint],
    extent_x_km: f32,
    extent_y_km: f32,
    stroke: egui::Stroke,
) {
    let projected: Vec<egui::Pos2> = pts
        .iter()
        .filter_map(|p| {
            let elev = srtm_stream::sample_elevation_m(selected_root, *p).unwrap_or(0.0) + 3.0;
            projection::project_local(layout, view, focus, *p, elev, extent_x_km, extent_y_km)
        })
        .map(|pp| pp.pos)
        .collect();
    if projected.len() >= 2 {
        painter.add(egui::Shape::line(projected, stroke));
    }
}

pub(super) fn local_geo_bounds(center: GeoPoint, view_zoom: f32) -> OsmGeoBounds {
    let half_extent_deg = visual_half_extent_for_zoom(view_zoom);
    OsmGeoBounds {
        min_lat: (center.lat - half_extent_deg).clamp(-85.0511, 85.0511),
        max_lat: (center.lat + half_extent_deg).clamp(-85.0511, 85.0511),
        min_lon: (center.lon - half_extent_deg).clamp(-180.0, 180.0),
        max_lon: (center.lon + half_extent_deg).clamp(-180.0, 180.0),
    }
}

pub(super) fn road_tile_zoom(render_zoom: f32) -> u8 {
    if render_zoom >= 10.0 {
        10
    } else if render_zoom >= 6.0 {
        8
    } else if render_zoom >= 3.5 {
        6
    } else {
        4
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_projection_expands_paris_contour_stack() {
        let model = crate::model::AppModel::seed_demo();
        let event = model.selected_event().expect("selected event");
        let render_zoom = 6.0;
        let Some(contours) = (0..20).find_map(|_| {
            let contours = contour_asset::load_srtm_region_for_view(
                model.selected_root.as_deref(),
                event.location,
                event.location,
                render_zoom,
                2,
                egui::Context::default(),
            );
            if contours.is_none() {
                std::thread::sleep(std::time::Duration::from_millis(150));
            }
            contours
        }) else {
            return;
        };
        let layout = layout(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1200.0, 900.0),
        ));
        let half_extent_deg = srtm_focus_cache::half_extent_for_zoom(render_zoom);
        let km_per_deg_lat = 111.32f32;
        let km_per_deg_lon = km_per_deg_lat * event.location.lat.to_radians().cos().abs().max(0.2);
        let extent_x_km = (half_extent_deg * km_per_deg_lon).max(1.0);
        let extent_y_km = (half_extent_deg * km_per_deg_lat).max(1.0);

        let points: Vec<_> = contours
            .iter()
            .flat_map(|contour| {
                contour
                    .points
                    .iter()
                    .map(move |point| (*point, contour.elevation_m))
            })
            .filter_map(|(point, elevation_m)| {
                projection::project_local(
                    &layout,
                    &model.globe_view,
                    event.location,
                    point,
                    elevation_m,
                    extent_x_km,
                    extent_y_km,
                )
            })
            .collect();

        let min_x = points
            .iter()
            .map(|point| point.pos.x)
            .fold(f32::INFINITY, f32::min);
        let max_x = points
            .iter()
            .map(|point| point.pos.x)
            .fold(f32::NEG_INFINITY, f32::max);
        let min_y = points
            .iter()
            .map(|point| point.pos.y)
            .fold(f32::INFINITY, f32::min);
        let max_y = points
            .iter()
            .map(|point| point.pos.y)
            .fold(f32::NEG_INFINITY, f32::max);

        assert!(!points.is_empty());
        assert!(max_x - min_x > 180.0);
        assert!(max_y - min_y > 140.0);
    }
}
