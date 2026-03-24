use crate::model::{AppModel, EventRecord, GeoPoint, GlobeViewState, NearbyCamera};
use crate::osm_ingest::{self, GeoBounds as OsmGeoBounds, RoadLayerKind, RoadPolyline, WaterPolyline};
use crate::terrain_assets;
use crate::theme;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use super::contour_asset;
use super::globe_scene::GlobeScene;
use super::local_terrain_pass;
use super::srtm_focus_cache;
use super::srtm_stream;

#[allow(dead_code)]
pub const LOCAL_TRANSITION_START_ZOOM: f32 = 4.0;
#[allow(dead_code)]
pub const LOCAL_MODE_MIN_ZOOM: f32 = 25.0;
const LOCAL_STREAM_RADIUS: i32 = 2;
const BASE_VERTICAL_EXAGGERATION: f32 = 2.1;

struct LocalLayout {
    center: egui::Pos2,
    focus_center: egui::Pos2,
    width: f32,
    height: f32,
    horizontal_scale: f32,
}

#[derive(Clone, Copy)]
struct ProjectedLocalPoint {
    pos: egui::Pos2,
    depth: f32,
}

pub fn paint(painter: &egui::Painter, rect: egui::Rect, model: &AppModel, time: f64) -> GlobeScene {
    painter.rect_filled(rect, 12.0, theme::canvas_background());
    if !model.cinematic_mode {
        draw_frame(painter, rect);
    }

    let layout = layout(rect);
    let Some(focus) = model.terrain_focus_location() else {
        draw_empty_state(painter, rect, "No terrain focus selected");
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

    let contours = contour_asset::load_srtm_region_for_view(
        model.selected_root.as_deref(),
        focus,
        viewport_center,
        render_zoom,
        LOCAL_STREAM_RADIUS,
        painter.ctx().clone(),
    );
    let cache_status = srtm_focus_cache::focus_contour_region_status(
        model.selected_root.as_deref(),
        viewport_center,
        render_zoom,
        LOCAL_STREAM_RADIUS,
    );

    let nearby = if model.focused_city().is_none() {
        model.nearby_cameras(250.0)
    } else {
        Vec::new()
    };

    // Pulsing tile-grid glow: only draw cells that are NOT yet ready in the cache.
    let still_loading = cache_status
        .map(|s| s.ready_assets < s.total_assets)
        .unwrap_or(contours.is_none());
    if still_loading {
        let ready_buckets = srtm_focus_cache::ready_tile_buckets(
            model.selected_root.as_deref(),
            viewport_center,
            render_zoom,
            LOCAL_STREAM_RADIUS,
        );
        draw_tile_pulse_grid(
            painter,
            &layout,
            &model.globe_view,
            viewport_center,
            render_zoom,
            LOCAL_STREAM_RADIUS,
            time,
            &ready_buckets,
        );
    }

    let contours_slice = contours.as_ref().map(|v| v.as_slice()).unwrap_or(&[]);
    if !contours_slice.is_empty() {
        draw_contour_stack(
            painter,
            &layout,
            &model.globe_view,
            viewport_center,
            render_zoom,
            contours_slice,
            1.0,
        );
        draw_roads(
            painter,
            &layout,
            &model.globe_view,
            model.selected_root.as_deref(),
            viewport_center,
            render_zoom,
            model.show_major_roads,
            model.show_minor_roads,
        );
        draw_water(
            painter,
            &layout,
            &model.globe_view,
            model.selected_root.as_deref(),
            viewport_center,
            render_zoom,
            model.show_water,
        );
    }

    // ── GPU terrain surface ────────────────────────────────────────────────
    // Rendered on top of contours/roads so the shaded mesh occludes the line
    // work within the terrain quad — outside the quad the contours remain fully
    // visible, giving a high-contrast look everywhere else.
    // Only added when the GPU heightmap is actually uploaded; while building
    // we let the contours + loading animation show through unobstructed.
    let terrain_ready = local_terrain_pass::is_heightmap_ready(
        viewport_center,
        visual_half_extent_for_zoom(model.globe_view.local_zoom),
        model.selected_root.as_deref(),
    );
    if model.show_terrain_surface && terrain_ready {
        let half_extent_deg = visual_half_extent_for_zoom(model.globe_view.local_zoom);
        let terrain_layout = local_terrain_pass::LocalTerrainLayout {
            focus_center:    layout.focus_center,
            horizontal_scale: layout.horizontal_scale,
            height:          layout.height,
        };
        let callback = local_terrain_pass::LocalTerrainCallback::new(
            viewport_center,
            half_extent_deg,
            &terrain_layout,
            model.globe_view.local_yaw,
            model.globe_view.local_pitch,
            model.globe_view.local_layer_spread,
            0.95,
            model.selected_root.as_deref(),
            theme::scene_backdrop(),  // sea / deep
            theme::topo_color(),      // low land
            theme::contour_color(),   // mid / high land
            theme::hot_color(),       // peaks
        );
        painter.add(callback.into_paint_callback(rect));
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
    let km_per_deg_lon =
        km_per_deg_lat * viewport_center.lat.to_radians().cos().abs().max(0.2);
    let extent_x_km = (half_extent_deg * km_per_deg_lon).max(1.0);
    let extent_y_km = (half_extent_deg * km_per_deg_lat).max(1.0);

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
                    let elev = marker_elevation_m(model.selected_root.as_deref(), event.location);
                    let ground = project_local(
                        &layout, &model.globe_view, viewport_center,
                        event.location, elev, extent_x_km, extent_y_km,
                    )?;
                    // Tip: project the same point 1 km higher, then cap the
                    // screen-space length so beams don't vary wildly with tilt.
                    let tip = project_local(
                        &layout, &model.globe_view, viewport_center,
                        event.location, elev + 1000.0, extent_x_km, extent_y_km,
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
                    draw_event_marker(
                        painter, ground, tip, event,
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
            project_local(
                &layout, &model.globe_view, viewport_center,
                camera.location,
                marker_elevation_m(model.selected_root.as_deref(), camera.location),
                extent_x_km, extent_y_km,
            )
            .map(|projected| {
                draw_camera_marker(
                    painter, projected,
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
    draw_camera_links(painter, anchor, &camera_markers);
    if model.show_coastlines {
        draw_coastlines_local(
            painter,
            &layout,
            &model.globe_view,
            viewport_center,
            render_zoom,
            model.selected_root.as_deref(),
        );
    }
    if model.show_bathymetry {
        draw_bathymetry_local(
            painter,
            &layout,
            &model.globe_view,
            viewport_center,
            render_zoom,
            model.selected_root.as_deref(),
        );
    }
    draw_legend(painter, rect, "LOCAL EVENT TERRAIN", render_zoom);
    draw_progress_overlay(
        painter,
        rect,
        cache_status,
        osm_ingest::osmium_cell_progress(),
        osm_ingest::active_job_note().as_deref(),
    );

    GlobeScene {
        event_markers,
        camera_markers,
        ship_markers: Vec::new(),
        flight_markers: Vec::new(),
        arcgis_feature_markers: Vec::new(),
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
    );
}

pub fn is_active(model: &AppModel) -> bool {
    model.globe_view.local_mode
        && model.terrain_focus_location().is_some()
        && terrain_assets::find_srtm_root(model.selected_root.as_deref()).is_some()
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

    let render_zoom = local_render_zoom(model.globe_view.local_zoom);
    srtm_focus_cache::focus_contour_region_status(
        model.selected_root.as_deref(),
        model.globe_view.local_center,
        render_zoom,
        LOCAL_STREAM_RADIUS,
    )
    .map(|status| status.ready_assets < status.total_assets)
    // None means the cache DB doesn't exist yet — tiles are definitely not loaded.
    .unwrap_or(true)
}

// Minimum local zoom value — allows zooming out to ~500 km half-span.
pub const LOCAL_ZOOM_MIN: f32 = 1.0;

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
    let ground = project_local(
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

fn draw_frame(painter: &egui::Painter, rect: egui::Rect) {
    painter.rect_stroke(
        rect.shrink(6.0),
        12.0,
        egui::Stroke::new(0.7, theme::topo_color().gamma_multiply(0.45)),
        egui::StrokeKind::Outside,
    );

    for &(x, y, x_dir, y_dir) in &[
        (rect.left() + 18.0, rect.top() + 18.0, 28.0, 16.0),
        (rect.right() - 18.0, rect.top() + 18.0, -28.0, 16.0),
        (rect.left() + 18.0, rect.bottom() - 18.0, 28.0, -16.0),
        (rect.right() - 18.0, rect.bottom() - 18.0, -28.0, -16.0),
    ] {
        painter.line_segment(
            [egui::pos2(x, y), egui::pos2(x + x_dir, y)],
            egui::Stroke::new(1.0, theme::topo_color()),
        );
        painter.line_segment(
            [egui::pos2(x, y), egui::pos2(x, y + y_dir)],
            egui::Stroke::new(1.0, theme::topo_color()),
        );
    }
}

/// Draws a pulsing glow over every tile footprint in the expected load grid.
/// Each tile is projected as a quadrilateral matching its actual geo-extent so
/// the placeholder fills exactly the area that will be covered by contour lines
/// once the tile finishes building.  Tiles with loaded geometry naturally cover
/// their pulse; pending tiles keep glowing until data arrives.
/// Spectral dissolve: each pending tile is subdivided into an 8×8 grid of
/// cells.  A time-based "cursor" sweeps 0→1 over DISSOLVE_CYCLE seconds;
/// each cell has a deterministic random threshold and disappears when the
/// cursor passes it.  Cells near their threshold glow with `hot_color` and
/// get a chromatic-aberration fringe drawn in the theme pair colors.
fn draw_tile_pulse_grid(
    painter: &egui::Painter,
    layout: &LocalLayout,
    view: &GlobeViewState,
    viewport_center: GeoPoint,
    render_zoom: f32,
    radius: i32,
    time: f64,
    ready_buckets: &std::collections::HashSet<(i32, i32)>,
) {
    const GRID: usize = 50;          // 50×50 = 2 500 cells per tile
    const DISSOLVE_CYCLE: f64 = 7.0; // seconds for one full sweep
    const EDGE_BAND: f32 = 0.14;    // fraction of cycle that counts as "burning"
    const CELL_INSET: f32 = 0.10;   // fractional gap between cells (10% each side)

    let half_extent = srtm_focus_cache::half_extent_for_zoom(render_zoom);
    let bucket_step = half_extent * 0.45;
    let visual_half = visual_half_extent_for_zoom(view.local_zoom);
    let km_per_deg_lat = 111.32f32;
    let km_per_deg_lon =
        km_per_deg_lat * viewport_center.lat.to_radians().cos().abs().max(0.2);
    let extent_x_km = (visual_half * km_per_deg_lon).max(1.0);
    let extent_y_km = (visual_half * km_per_deg_lat).max(1.0);

    let center_lat_b = (viewport_center.lat / bucket_step).round() as i32;
    let center_lon_b = (viewport_center.lon / bucket_step).round() as i32;
    let half = half_extent;

    // Dissolve cursor: 0 (all cells visible) → 1 (all gone), then resets.
    let cursor = ((time % DISSOLVE_CYCLE) / DISSOLVE_CYCLE) as f32;

    // Gentle global breath layered on top so nothing ever feels static.
    let breath = ((time as f32 * std::f32::consts::TAU / 4.5).sin() * 0.5 + 0.5) * 0.35 + 0.65;

    // Theme colours — hot_color for the burning edge, contour_color for the
    // calm interior.  The CA fringe uses both so it themes automatically.
    let [cr, cg, cb, _] = theme::contour_color().to_array();
    let [hr, hg, hb, _] = theme::hot_color().to_array();

    // One mesh for the main cells, two for the chromatic fringe ghosts.
    let mut mesh = egui::Mesh::default();
    let mut mesh_hot = egui::Mesh::default(); // shifted toward hot_color
    let mut mesh_cnt = egui::Mesh::default(); // shifted toward contour_color

    for dlat in -radius..=radius {
        for dlon in -radius..=radius {
            let lat_b = center_lat_b + dlat;
            let lon_b = center_lon_b + dlon;
            if ready_buckets.contains(&(lat_b, lon_b)) {
                continue;
            }

            let tile_lat = (lat_b as f32 * bucket_step).clamp(-89.9, 89.9);
            let tile_lon = lon_b as f32 * bucket_step;

            // Project the 4 geo corners → screen space (NW, NE, SE, SW).
            let geo_corners = [
                GeoPoint { lat: tile_lat + half, lon: tile_lon - half },
                GeoPoint { lat: tile_lat + half, lon: tile_lon + half },
                GeoPoint { lat: tile_lat - half, lon: tile_lon + half },
                GeoPoint { lat: tile_lat - half, lon: tile_lon - half },
            ];
            let sc: Vec<egui::Pos2> = geo_corners
                .iter()
                .filter_map(|&c| {
                    project_local(layout, view, viewport_center, c, 0.0,
                                  extent_x_km, extent_y_km)
                })
                .map(|p| p.pos)
                .collect();
            if sc.len() < 4 {
                continue;
            }
            let (nw, ne, se, sw) = (sc[0], sc[1], sc[2], sc[3]);

            let seed = tile_hash(lat_b, lon_b);

            for row in 0..GRID {
                for col in 0..GRID {
                    let threshold = cell_rand(seed, row, col);
                    if threshold < cursor {
                        continue; // this cell has dissolved
                    }

                    // 1.0 = far from dissolving, 0.0 = about to vanish
                    let edge = ((threshold - cursor) / EDGE_BAND).clamp(0.0, 1.0);

                    // Bilinear sub-quad with a tiny inset gap.
                    let n = GRID as f32;
                    let u0 = col as f32 / n + CELL_INSET / n;
                    let u1 = (col as f32 + 1.0) / n - CELL_INSET / n;
                    let v0 = row as f32 / n + CELL_INSET / n;
                    let v1 = (row as f32 + 1.0) / n - CELL_INSET / n;

                    let p_nw = bilerp(nw, ne, sw, se, u0, v0);
                    let p_ne = bilerp(nw, ne, sw, se, u1, v0);
                    let p_se = bilerp(nw, ne, sw, se, u1, v1);
                    let p_sw = bilerp(nw, ne, sw, se, u0, v1);

                    // Mix contour→hot as cell approaches its threshold.
                    let mix = (1.0 - edge).powf(1.8);
                    let r = lerp_u8(cr, hr, mix);
                    let g = lerp_u8(cg, hg, mix);
                    let b = lerp_u8(cb, hb, mix);
                    let alpha = (lerp_f32(8.0, 80.0, 1.0 - edge) * breath) as u8;
                    quad(&mut mesh, p_nw, p_ne, p_se, p_sw,
                         egui::Color32::from_rgba_unmultiplied(r, g, b, alpha));

                    // Chromatic-aberration fringe on burning-edge cells.
                    if edge < 0.4 {
                        let t = 1.0 - edge / 0.4; // 0→1 as cell nears threshold
                        let fa = (t * 40.0 * breath) as u8;
                        let offset = egui::Vec2::new(t * 1.8, 0.0);

                        // Hot ghost shifted one way
                        quad(&mut mesh_hot,
                             p_nw + offset, p_ne + offset,
                             p_se + offset, p_sw + offset,
                             egui::Color32::from_rgba_unmultiplied(hr, hg, hb, fa));
                        // Contour ghost shifted the other way
                        quad(&mut mesh_cnt,
                             p_nw - offset, p_ne - offset,
                             p_se - offset, p_sw - offset,
                             egui::Color32::from_rgba_unmultiplied(cr, cg, cb, fa));
                    }
                }
            }
        }
    }

    if !mesh.vertices.is_empty()     { painter.add(egui::Shape::mesh(mesh)); }
    if !mesh_hot.vertices.is_empty() { painter.add(egui::Shape::mesh(mesh_hot)); }
    if !mesh_cnt.vertices.is_empty() { painter.add(egui::Shape::mesh(mesh_cnt)); }
}

// ── tile-dissolve helpers ──────────────────────────────────────────────────

/// Bilinear interpolation across a screen-space quad.
/// Corners: NW (u=0,v=0), NE (u=1,v=0), SW (u=0,v=1), SE (u=1,v=1).
#[inline]
fn bilerp(nw: egui::Pos2, ne: egui::Pos2, sw: egui::Pos2, se: egui::Pos2,
          u: f32, v: f32) -> egui::Pos2 {
    nw.lerp(ne, u).lerp(sw.lerp(se, u), v)
}

/// Append a solid-colour quad (two triangles) to `mesh`.
#[inline]
fn quad(mesh: &mut egui::Mesh,
        nw: egui::Pos2, ne: egui::Pos2, se: egui::Pos2, sw: egui::Pos2,
        color: egui::Color32) {
    let i = mesh.vertices.len() as u32;
    mesh.colored_vertex(nw, color);
    mesh.colored_vertex(ne, color);
    mesh.colored_vertex(se, color);
    mesh.colored_vertex(sw, color);
    mesh.add_triangle(i, i + 1, i + 2);
    mesh.add_triangle(i, i + 2, i + 3);
}

/// Stable per-tile seed from bucket coordinates.
#[inline]
fn tile_hash(lat_b: i32, lon_b: i32) -> u64 {
    let a = (lat_b as u64) & 0xFFFF_FFFF;
    let b = (lon_b as u64) & 0xFFFF_FFFF;
    a.wrapping_mul(2_654_435_761)
        .wrapping_add(b.wrapping_mul(2_246_822_519))
        .wrapping_mul(6_364_136_223_846_793_005)
}

/// Deterministic float in [0, 1) for a given tile seed + (row, col).
///
/// Uses a splitmix64-style finalizer: row and col are mixed into the seed
/// with different primes *before* the avalanche pass, so adjacent cells
/// produce completely uncorrelated values rather than an arithmetic sequence.
#[inline]
fn cell_rand(seed: u64, row: usize, col: usize) -> f32 {
    let mut x = seed
        .wrapping_add((row as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15))
        .wrapping_add((col as u64).wrapping_mul(0x6c62_272e_07bb_0142));
    // splitmix64 avalanche
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^= x >> 31;
    (x >> 40) as f32 / (1u64 << 24) as f32
}

#[inline]
fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t).clamp(0.0, 255.0) as u8
}

#[inline]
fn lerp_f32(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn draw_contour_stack(
    painter: &egui::Painter,
    layout: &LocalLayout,
    view: &GlobeViewState,
    focus: GeoPoint,
    _render_zoom: f32,
    contours: &[contour_asset::ContourPath],
    alpha: f32,
) {
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
                project_local(
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

        let major = (contour.elevation_m.round() as i32).rem_euclid(50) == 0;
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

// ── Road tile cache ────────────────────────────────────────────────────────
// Road geometry is fetched from SQLite and cached until the tile coverage
// actually changes.  Opening a DB connection + running a query on every
// frame was the source of the 2-5 FPS regression.

/// A road polyline with elevation pre-sampled for every vertex.
/// Elevation is computed once at cache-load time so `draw_road_layer`
/// only has to do fast projection math on each frame.
struct ElevatedRoad {
    points: Vec<(GeoPoint, f32)>, // (position, elevation_m above ground)
}

impl ElevatedRoad {
    /// Build an elevated road, sampling SRTM elevation at every `elev_step`-th
    /// vertex and linearly interpolating the rest.  Use `elev_step = 1` for
    /// major roads (full fidelity) and a larger value for minor roads to cap
    /// the number of expensive per-point SRTM lookups.
    fn from_polyline(
        poly: &osm_ingest::RoadPolyline,
        selected_root: Option<&Path>,
        elev_step: usize,
    ) -> Self {
        let pts = &poly.points;
        let n = pts.len();
        if n == 0 {
            return Self { points: Vec::new() };
        }
        let step = elev_step.max(1);

        // Sample elevation at every `step`-th index (always including last).
        let mut sampled: Vec<(usize, f32)> = (0..n)
            .step_by(step)
            .map(|i| {
                let e = srtm_stream::sample_elevation_m(selected_root, pts[i]).unwrap_or(0.0) + 3.0;
                (i, e)
            })
            .collect();
        if sampled.last().map(|&(i, _)| i) != Some(n - 1) {
            let e = srtm_stream::sample_elevation_m(selected_root, pts[n - 1]).unwrap_or(0.0) + 3.0;
            sampled.push((n - 1, e));
        }

        // Linearly interpolate elevations for skipped vertices.
        let mut elevations = vec![0.0f32; n];
        for w in sampled.windows(2) {
            let (i0, e0) = w[0];
            let (i1, e1) = w[1];
            for i in i0..=i1 {
                let t = if i1 > i0 { (i - i0) as f32 / (i1 - i0) as f32 } else { 0.0 };
                elevations[i] = e0 + (e1 - e0) * t;
            }
        }

        let points = pts.iter().zip(elevations).map(|(&pt, e)| (pt, e)).collect();
        Self { points }
    }
}

/// Clear the road tile cache so the next draw reloads from SQLite.
/// Call this whenever the road layer checkboxes change.
pub fn invalidate_road_cache() {
    if let Ok(mut g) = road_cache().lock() {
        g.cache = None;
        // Leave `building` alone — any in-flight thread will finish and
        // write a result; the stale check will then trigger a fresh build.
    }
}

/// True while a background road-cache build is in progress.
pub fn road_cache_building() -> bool {
    road_cache().lock().map(|g| g.building).unwrap_or(false)
}

struct RoadCache {
    tile_zoom: u8,
    tile_x_min: u32,
    tile_x_max: u32,
    tile_y_min: u32,
    tile_y_max: u32,
    road_gen: u64,
    had_major: bool,
    had_minor: bool,
    /// Raw geometry from SQLite — built in a background thread (no SRTM I/O).
    major_polys: Vec<RoadPolyline>,
    minor_polys: Vec<RoadPolyline>,
    /// Elevation-enriched roads, populated lazily on first render so that SRTM
    /// tiles are guaranteed to be in the hot tile-LRU when we sample them.
    major_elevated: Option<Vec<ElevatedRoad>>,
    minor_elevated: Option<Vec<ElevatedRoad>>,
}

struct RoadCacheStore {
    cache: Option<RoadCache>,
    /// True while a background thread is building new geometry.
    building: bool,
}

fn road_cache() -> &'static Mutex<RoadCacheStore> {
    static CACHE: OnceLock<Mutex<RoadCacheStore>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(RoadCacheStore { cache: None, building: false }))
}

fn draw_roads(
    painter: &egui::Painter,
    layout: &LocalLayout,
    view: &GlobeViewState,
    selected_root: Option<&Path>,
    viewport_center: GeoPoint,
    render_zoom: f32,
    show_major_roads: bool,
    show_minor_roads: bool,
) {
    if !show_major_roads && !show_minor_roads {
        if let Ok(mut g) = road_cache().lock() { g.cache = None; }
        return;
    }

    let bounds = local_geo_bounds(viewport_center, view.local_zoom);
    let tile_zoom = road_tile_zoom(render_zoom);
    let half_extent_deg = visual_half_extent_for_zoom(view.local_zoom);
    let km_per_deg_lat = 111.32f32;
    let km_per_deg_lon = km_per_deg_lat * viewport_center.lat.to_radians().cos().abs().max(0.2);
    let extent_x_km = (half_extent_deg * km_per_deg_lon).max(1.0);
    let extent_y_km = (half_extent_deg * km_per_deg_lat).max(1.0);

    let (x0, y0) = osm_ingest::lat_lon_to_tile(bounds.max_lat, bounds.min_lon, tile_zoom);
    let (x1, y1) = osm_ingest::lat_lon_to_tile(bounds.min_lat, bounds.max_lon, tile_zoom);
    let (txmin, txmax) = (x0.min(x1), x0.max(x1));
    let (tymin, tymax) = (y0.min(y1), y0.max(y1));
    const MARGIN: u32 = 1;
    let current_gen = osm_ingest::road_data_generation();

    // ── Stale check + background build launch ─────────────────────────────
    {
        let mut store = match road_cache().lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        let stale = store.cache.as_ref().map_or(true, |c| {
            c.tile_zoom != tile_zoom
                || c.road_gen != current_gen
                || c.had_major != show_major_roads
                || c.had_minor != show_minor_roads
                || c.tile_x_min > txmin
                || c.tile_x_max < txmax
                || c.tile_y_min > tymin
                || c.tile_y_max < tymax
        });

        if stale && !store.building {
            let (lxmin, lxmax) = (txmin.saturating_sub(MARGIN), txmax + MARGIN);
            let (lymin, lymax) = (tymin.saturating_sub(MARGIN), tymax + MARGIN);
            store.building = true;
            drop(store); // release lock before spawning

            // No SRTM I/O in the background thread — geometry only.
            // Elevation is sampled lazily on the first render call so that the
            // SRTM tile LRU is guaranteed warm (contours have already loaded tiles).
            let root_buf = selected_root.map(|p| p.to_path_buf());
            std::thread::spawn(move || {
                let root_ref = root_buf.as_deref();
                let major_polys = if show_major_roads {
                    osm_ingest::load_roads_for_bounds(root_ref, bounds, tile_zoom, RoadLayerKind::Major)
                } else { Vec::new() };
                let minor_polys = if show_minor_roads {
                    osm_ingest::load_roads_for_bounds(root_ref, bounds, tile_zoom, RoadLayerKind::Minor)
                } else { Vec::new() };

                if let Ok(mut store) = road_cache().lock() {
                    store.cache = Some(RoadCache {
                        tile_zoom,
                        tile_x_min: lxmin, tile_x_max: lxmax,
                        tile_y_min: lymin, tile_y_max: lymax,
                        road_gen: current_gen,
                        had_major: show_major_roads,
                        had_minor: show_minor_roads,
                        major_polys,
                        minor_polys,
                        major_elevated: None,
                        minor_elevated: None,
                    });
                    store.building = false;
                }
                crate::app::request_repaint();
            });
        }
        // `store` dropped here (or already explicitly dropped above)
    }

    // ── Render from whatever cache is currently ready ───────────────────
    // Lazily build elevation-enriched roads on first render after a cache
    // update.  By now the SRTM tile LRU is warm (contour rendering already
    // loaded the tiles), so every sample_elevation_m call hits the in-memory
    // tile cache instead of disk.
    let mut store = match road_cache().lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    let Some(cache) = &mut store.cache else { return };

    if show_major_roads && cache.major_elevated.is_none() {
        cache.major_elevated = Some(
            cache.major_polys.iter()
                .map(|p| ElevatedRoad::from_polyline(p, selected_root, 1))
                .collect(),
        );
    }
    if show_minor_roads && cache.minor_elevated.is_none() {
        cache.minor_elevated = Some(
            cache.minor_polys.iter()
                .map(|p| ElevatedRoad::from_polyline(p, selected_root, 5))
                .collect(),
        );
    }

    if show_minor_roads {
        if let Some(minor) = &cache.minor_elevated {
            draw_road_layer(painter, layout, view, viewport_center,
                extent_x_km, extent_y_km, minor,
                egui::Stroke::new(0.8, egui::Color32::from_rgb(116, 132, 142)));
        }
    }
    if show_major_roads {
        if let Some(major) = &cache.major_elevated {
            draw_road_layer(painter, layout, view, viewport_center,
                extent_x_km, extent_y_km, major,
                egui::Stroke::new(1.35, egui::Color32::from_rgb(255, 210, 92)));
        }
    }
}

fn draw_road_layer(
    painter: &egui::Painter,
    layout: &LocalLayout,
    view: &GlobeViewState,
    viewport_center: GeoPoint,
    extent_x_km: f32,
    extent_y_km: f32,
    roads: &[ElevatedRoad],
    stroke: egui::Stroke,
) {
    for road in roads {
        let points: Vec<_> = road
            .points
            .iter()
            .filter_map(|&(pt, elev)| {
                // Elevation is already pre-sampled — this is pure projection math.
                project_local(layout, view, viewport_center, pt, elev,
                              extent_x_km, extent_y_km)
                    .map(|p| p.pos)
            })
            .collect();

        if points.len() >= 2 {
            painter.add(egui::Shape::line(points, stroke));
        }
    }
}

// ── Water layer ────────────────────────────────────────────────────────────────
//
// Mirrors the road layer architecture: a static WaterCache holds pre-projected
// vertices so that `draw_water` is pure painter calls on every frame.

/// A water feature with elevation pre-sampled at every vertex.
struct ElevatedWater {
    points: Vec<(GeoPoint, f32)>, // (position, elevation_m)
    is_area: bool,
}

impl ElevatedWater {
    fn from_polyline(poly: &WaterPolyline, selected_root: Option<&Path>) -> Self {
        let pts = &poly.points;
        let n = pts.len();
        if n == 0 {
            return Self { points: Vec::new(), is_area: poly.is_area };
        }
        // Sample every 4th vertex for water (large polygons can be huge).
        let step = 4usize;
        let mut sampled: Vec<(usize, f32)> = (0..n)
            .step_by(step)
            .map(|i| {
                let e = srtm_stream::sample_elevation_m(selected_root, pts[i]).unwrap_or(0.0) + 1.5;
                (i, e)
            })
            .collect();
        if sampled.last().map(|&(i, _)| i) != Some(n - 1) {
            let e = srtm_stream::sample_elevation_m(selected_root, pts[n - 1]).unwrap_or(0.0) + 1.5;
            sampled.push((n - 1, e));
        }
        let mut elevations = vec![0.0f32; n];
        for w in sampled.windows(2) {
            let (i0, e0) = w[0];
            let (i1, e1) = w[1];
            for i in i0..=i1 {
                let t = if i1 > i0 { (i - i0) as f32 / (i1 - i0) as f32 } else { 0.0 };
                elevations[i] = e0 + (e1 - e0) * t;
            }
        }
        let points = pts.iter().zip(elevations).map(|(&pt, e)| (pt, e)).collect();
        Self { points, is_area: poly.is_area }
    }
}

struct WaterCache {
    tile_zoom: u8,
    tile_x_min: u32,
    tile_x_max: u32,
    tile_y_min: u32,
    tile_y_max: u32,
    water_gen: u64,
    /// Raw geometry from SQLite — built in a background thread (no SRTM I/O).
    polys: Vec<WaterPolyline>,
    /// Elevation-enriched features, populated lazily on first render.
    features_elevated: Option<Vec<ElevatedWater>>,
}

struct WaterCacheStore {
    cache: Option<WaterCache>,
    building: bool,
}

fn water_cache() -> &'static Mutex<WaterCacheStore> {
    static CACHE: OnceLock<Mutex<WaterCacheStore>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(WaterCacheStore { cache: None, building: false }))
}

/// Clear the water tile cache so the next draw reloads from SQLite.
pub fn invalidate_water_cache() {
    if let Ok(mut g) = water_cache().lock() {
        g.cache = None;
    }
}

/// True while a background water-cache build is in progress.
pub fn water_cache_building() -> bool {
    water_cache().lock().map(|g| g.building).unwrap_or(false)
}

fn draw_water(
    painter: &egui::Painter,
    layout: &LocalLayout,
    view: &GlobeViewState,
    selected_root: Option<&Path>,
    viewport_center: GeoPoint,
    render_zoom: f32,
    show_water: bool,
) {
    if !show_water {
        if let Ok(mut g) = water_cache().lock() { g.cache = None; }
        return;
    }

    let bounds = local_geo_bounds(viewport_center, view.local_zoom);
    let tile_zoom = road_tile_zoom(render_zoom);
    let half_extent_deg = visual_half_extent_for_zoom(view.local_zoom);
    let km_per_deg_lat = 111.32f32;
    let km_per_deg_lon = km_per_deg_lat * viewport_center.lat.to_radians().cos().abs().max(0.2);
    let extent_x_km = (half_extent_deg * km_per_deg_lon).max(1.0);
    let extent_y_km = (half_extent_deg * km_per_deg_lat).max(1.0);

    let (x0, y0) = osm_ingest::lat_lon_to_tile(bounds.max_lat, bounds.min_lon, tile_zoom);
    let (x1, y1) = osm_ingest::lat_lon_to_tile(bounds.min_lat, bounds.max_lon, tile_zoom);
    let (txmin, txmax) = (x0.min(x1), x0.max(x1));
    let (tymin, tymax) = (y0.min(y1), y0.max(y1));
    const MARGIN: u32 = 1;
    let current_gen = osm_ingest::water_data_generation();

    // ── Stale check + background build launch ─────────────────────────────
    {
        let mut store = match water_cache().lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        let stale = store.cache.as_ref().map_or(true, |c| {
            c.tile_zoom != tile_zoom
                || c.water_gen != current_gen
                || c.tile_x_min > txmin
                || c.tile_x_max < txmax
                || c.tile_y_min > tymin
                || c.tile_y_max < tymax
        });

        if stale && !store.building {
            let (lxmin, lxmax) = (txmin.saturating_sub(MARGIN), txmax + MARGIN);
            let (lymin, lymax) = (tymin.saturating_sub(MARGIN), tymax + MARGIN);
            store.building = true;
            drop(store);

            // No SRTM I/O in the background thread — geometry only.
            let root_buf = selected_root.map(|p| p.to_path_buf());
            std::thread::spawn(move || {
                let root_ref = root_buf.as_deref();
                let polys = osm_ingest::load_water_for_bounds(root_ref, bounds, tile_zoom);

                if let Ok(mut store) = water_cache().lock() {
                    store.cache = Some(WaterCache {
                        tile_zoom,
                        tile_x_min: lxmin, tile_x_max: lxmax,
                        tile_y_min: lymin, tile_y_max: lymax,
                        water_gen: current_gen,
                        polys,
                        features_elevated: None,
                    });
                    store.building = false;
                }
                crate::app::request_repaint();
            });
        }
    }

    // ── Render from whatever cache is currently ready ───────────────────
    // Lazily enrich with SRTM elevation on first render after a cache update.
    let mut store = match water_cache().lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    let Some(cache) = &mut store.cache else { return };

    if cache.features_elevated.is_none() {
        cache.features_elevated = Some(
            cache.polys.iter()
                .map(|p| ElevatedWater::from_polyline(p, selected_root))
                .collect(),
        );
    }
    let Some(features) = &cache.features_elevated else { return };

    let water_col = crate::theme::water_color();
    let line_stroke = egui::Stroke::new(1.2, water_col);

    for feat in features {
        let mut pts: Vec<_> = feat
            .points
            .iter()
            .filter_map(|&(pt, elev)| {
                project_local(layout, view, viewport_center, pt, elev,
                              extent_x_km, extent_y_km)
                    .map(|p| p.pos)
            })
            .collect();

        if pts.len() < 2 {
            continue;
        }

        // For area features (lakes, reservoirs) close the ring so it draws as a
        // loop.  Do NOT use convex_polygon — OSM shorelines are non-convex and
        // the fan triangulation produces the sharp spike artifacts seen in the
        // screenshots.
        if feat.is_area && pts.len() >= 3 {
            pts.push(pts[0]);
        }
        painter.add(egui::Shape::line(pts, line_stroke));
    }
}

#[allow(dead_code)]
fn draw_markers(
    painter: &egui::Painter,
    layout: &LocalLayout,
    view: &GlobeViewState,
    selected_root: Option<&Path>,
    viewport_center: GeoPoint,
    _render_zoom: f32,
    event: &EventRecord,
    nearby: &[NearbyCamera],
    selected_event_id: Option<&str>,
    selected_camera_id: Option<&str>,
    time: f64,
) -> (Vec<(String, egui::Pos2)>, Vec<(String, egui::Pos2)>) {
    let half_extent_deg = visual_half_extent_for_zoom(view.local_zoom);
    let km_per_deg_lat = 111.32f32;
    let km_per_deg_lon = km_per_deg_lat * viewport_center.lat.to_radians().cos().abs().max(0.2);
    let extent_x_km = (half_extent_deg * km_per_deg_lon).max(1.0);
    let extent_y_km = (half_extent_deg * km_per_deg_lat).max(1.0);

    let event_elev = marker_elevation_m(selected_root, event.location);
    let event_marker = project_local(
        layout,
        view,
        viewport_center,
        event.location,
        event_elev,
        extent_x_km,
        extent_y_km,
    );
    // Compute the screen-space "up" direction by projecting the same point at
    // a higher elevation and measuring the displacement.  Normalising then
    // scaling gives a beam of consistent pixel length regardless of zoom.
    let event_sky = project_local(
        layout, view, viewport_center, event.location,
        event_elev + 1000.0, extent_x_km, extent_y_km,
    );
    if let Some(event_marker) = event_marker {
        let tip = event_sky.map(|sky| {
            let dx = sky.pos.x - event_marker.pos.x;
            let dy = sky.pos.y - event_marker.pos.y;
            let len = (dx * dx + dy * dy).sqrt().max(0.1);
            egui::pos2(
                event_marker.pos.x + dx / len * EVENT_BEAM_HEIGHT_PX,
                event_marker.pos.y + dy / len * EVENT_BEAM_HEIGHT_PX,
            )
        }).unwrap_or(egui::pos2(event_marker.pos.x, event_marker.pos.y - EVENT_BEAM_HEIGHT_PX));

        draw_event_marker(
            painter,
            event_marker,
            tip,
            event,
            selected_event_id == Some(event.id.as_str()),
            time,
        );
    }

    let camera_markers = nearby
        .iter()
        .filter_map(|camera| {
            project_local(
                layout,
                view,
                viewport_center,
                camera.location,
                marker_elevation_m(selected_root, camera.location),
                extent_x_km,
                extent_y_km,
            )
            .map(|projected| {
                draw_camera_marker(
                    painter,
                    projected,
                    selected_camera_id == Some(camera.id.as_str()),
                );
                (camera.id.clone(), projected.pos)
            })
        })
        .collect();

    (
        event_marker
            .map(|marker| vec![(event.id.clone(), marker.pos)])
            .unwrap_or_default(),
        camera_markers,
    )
}

fn draw_camera_links(
    painter: &egui::Painter,
    event_marker: Option<egui::Pos2>,
    camera_markers: &[(String, egui::Pos2)],
) {
    let Some(event_marker) = event_marker else {
        return;
    };

    for (_, marker) in camera_markers {
        painter.line_segment(
            [event_marker, *marker],
            egui::Stroke::new(0.75, theme::camera_color().gamma_multiply(0.32)),
        );
    }
}

/// Height in screen-space pixels of an event laser beam.
const EVENT_BEAM_HEIGHT_PX: f32 = 110.0;

/// Draw a Factal event as a glowing laser beam tapering to a point.
/// Identical visual treatment to globe_scene::draw_event_marker.
fn draw_event_marker(
    painter: &egui::Painter,
    ground: ProjectedLocalPoint,
    tip: egui::Pos2,
    event: &EventRecord,
    is_selected: bool,
    time: f64,
) {
    let col = event.severity.color();
    let dx = tip.x - ground.pos.x;
    let dy = tip.y - ground.pos.y;

    // ── Atmospheric halos — taper in width and alpha toward the tip ───────────
    const HALO_SEGS: u32 = 7;
    for i in 0..HALO_SEGS {
        let t0 = i as f32 / HALO_SEGS as f32;
        let t1 = (i + 1) as f32 / HALO_SEGS as f32;
        let tm = (t0 + t1) * 0.5;
        let a = (1.0 - tm).powi(2);
        let p0 = egui::pos2(ground.pos.x + dx * t0, ground.pos.y + dy * t0);
        let p1 = egui::pos2(ground.pos.x + dx * t1, ground.pos.y + dy * t1);
        painter.line_segment([p0, p1], egui::Stroke::new((22.0 * a).max(0.5), col.gamma_multiply(0.04 * a)));
        painter.line_segment([p0, p1], egui::Stroke::new((11.0 * a).max(0.5), col.gamma_multiply(0.08 * a)));
        painter.line_segment([p0, p1], egui::Stroke::new(( 4.5 * a).max(0.5), col.gamma_multiply(0.16 * a)));
    }

    // ── Tapering core — cubic fade, width narrows to a point ─────────────────
    const SEGS: u32 = 14;
    for i in 0..SEGS {
        let t0 = i as f32 / SEGS as f32;
        let t1 = (i + 1) as f32 / SEGS as f32;
        let tm = (t0 + t1) * 0.5;
        let falloff = 1.0 - tm;
        let alpha   = falloff.powi(3);
        let w_glow  = (4.0 * falloff.powf(0.7)).max(0.4);
        let w_core  = (1.7 * falloff.powf(0.7)).max(0.3);
        let p0 = egui::pos2(ground.pos.x + dx * t0, ground.pos.y + dy * t0);
        let p1 = egui::pos2(ground.pos.x + dx * t1, ground.pos.y + dy * t1);
        painter.line_segment([p0, p1], egui::Stroke::new(w_glow, col.gamma_multiply(alpha * 0.30)));
        painter.line_segment([p0, p1], egui::Stroke::new(w_core, col.gamma_multiply(alpha * 0.96)));
    }

    // ── Ground strike ─────────────────────────────────────────────────────────
    if is_selected {
        let pulse = 9.0 + ((time as f32 * 2.6).sin() + 1.0) * 3.2;
        painter.circle_stroke(
            ground.pos, pulse,
            egui::Stroke::new(1.3, theme::marker_glow_warm()),
        );
    }
    painter.circle_stroke(ground.pos, 5.5, egui::Stroke::new(3.5, col.gamma_multiply(0.10)));
    painter.circle_stroke(ground.pos, 4.8, egui::Stroke::new(1.1, col.gamma_multiply(0.60)));
    painter.circle_filled(ground.pos, 2.2, col);
}

fn draw_camera_marker(painter: &egui::Painter, marker: ProjectedLocalPoint, is_selected: bool) {
    let radius = 3.4 + marker.depth;
    let color = if is_selected { theme::marker_camera_ring() } else { theme::camera_color() };

    // Soft halo so cameras read against the terrain
    painter.circle_stroke(
        marker.pos, radius + 5.0,
        egui::Stroke::new(5.0, color.gamma_multiply(0.08)),
    );
    painter.circle_filled(marker.pos, radius, color);
    if is_selected {
        painter.circle_stroke(marker.pos, radius + 3.0, egui::Stroke::new(1.1, color));
    }
}

fn draw_legend(painter: &egui::Painter, rect: egui::Rect, title: &str, render_zoom: f32) {
    let interval_m = srtm_focus_cache::contour_interval_for_zoom(render_zoom);
    let half_extent_km = visual_half_extent_for_zoom(render_zoom) * 111.32;
    painter.text(
        egui::pos2(rect.left() + 24.0, rect.bottom() - 86.0),
        egui::Align2::LEFT_TOP,
        format!(
            "{title}\nFIXED OBLIQUE CAMERA\n{interval_m}M CONTOURS · {half_extent_km:.0}KM HALF-SPAN"
        ),
        egui::FontId::monospace(12.0),
        theme::text_muted(),
    );
}

/// Draw the bottom-right progress overlay.  Handles SRTM cache progress and
/// osmium cell-extraction progress as stacked cards; each card is only shown
/// when its data is available so they coexist without gaps when both are active.
fn draw_progress_overlay(
    painter: &egui::Painter,
    rect: egui::Rect,
    cache_status: Option<srtm_focus_cache::FocusContourRegionStatus>,
    osmium_progress: Option<(u32, u32)>,
    job_note: Option<&str>,
) {
    const CARD_W: f32 = 200.0;
    const CARD_H: f32 = 36.0;
    const GAP: f32 = 4.0;
    const RIGHT_MARGIN: f32 = 12.0;
    const BOTTOM_MARGIN: f32 = 12.0;

    let cache_active = cache_status
        .map(|s| s.total_assets > 0 && s.ready_assets < s.total_assets)
        .unwrap_or(false);
    let osmium_active = osmium_progress.is_some();

    if !cache_active && !osmium_active {
        return;
    }

    // Cards stack upward from the bottom.  Cache bar is always on bottom when both visible.
    let mut bottom_y = rect.bottom() - BOTTOM_MARGIN;

    // ── SRTM cache card ────────────────────────────────────────────────────
    if cache_active {
        let status = cache_status.unwrap();
        let progress = (status.ready_assets as f32 / status.total_assets as f32).clamp(0.0, 1.0);
        let frame = egui::Rect::from_min_size(
            egui::pos2(rect.right() - RIGHT_MARGIN - CARD_W, bottom_y - CARD_H),
            egui::vec2(CARD_W, CARD_H),
        );
        let bar = egui::Rect::from_min_size(
            frame.left_bottom() + egui::vec2(0.0, -10.0),
            egui::vec2(frame.width(), 6.0),
        );
        draw_progress_card(
            painter,
            frame,
            bar,
            &format!(
                "CACHE {} / {}  ·  {} PENDING",
                status.ready_assets, status.total_assets, status.pending_assets
            ),
            progress,
            theme::topo_color(),
        );
        bottom_y = frame.top() - GAP;
    }

    // ── Osmium cell-extraction card ────────────────────────────────────────
    if osmium_active {
        let (done, total) = osmium_progress.unwrap();
        let progress = if total > 0 { done as f32 / total as f32 } else { 0.0 };
        // Truncate job note to fit in card width (≈26 chars at monospace 11)
        let label = if let Some(note) = job_note {
            let trimmed = note.trim_end_matches('…').trim_end_matches("...");
            if trimmed.len() > 28 { format!("{}…", &trimmed[..28]) } else { trimmed.to_owned() }
        } else {
            format!("OSMIUM {done}/{total} cells")
        };
        let frame = egui::Rect::from_min_size(
            egui::pos2(rect.right() - RIGHT_MARGIN - CARD_W, bottom_y - CARD_H),
            egui::vec2(CARD_W, CARD_H),
        );
        let bar = egui::Rect::from_min_size(
            frame.left_bottom() + egui::vec2(0.0, -10.0),
            egui::vec2(frame.width(), 6.0),
        );
        draw_progress_card(
            painter,
            frame,
            bar,
            &label,
            progress,
            egui::Color32::from_rgb(160, 130, 50),
        );
    }
}

fn draw_progress_card(
    painter: &egui::Painter,
    frame: egui::Rect,
    bar: egui::Rect,
    label: &str,
    progress: f32,
    fill_color: egui::Color32,
) {
    painter.rect_filled(frame, 6.0, theme::panel_fill(208));
    painter.rect_stroke(
        frame,
        6.0,
        egui::Stroke::new(1.0, theme::panel_stroke()),
        egui::StrokeKind::Outside,
    );
    painter.text(
        frame.left_top() + egui::vec2(8.0, 6.0),
        egui::Align2::LEFT_TOP,
        label,
        egui::FontId::monospace(10.5),
        theme::text_muted(),
    );
    painter.rect_filled(bar, 3.0, theme::panel_fill(230).gamma_multiply(2.5));
    if progress > 0.0 {
        let filled = egui::Rect::from_min_max(
            bar.min,
            egui::pos2(bar.left() + bar.width() * progress, bar.bottom()),
        );
        painter.rect_filled(filled, 3.0, fill_color);
    }
}

fn draw_bathymetry_local(
    painter: &egui::Painter,
    layout: &LocalLayout,
    view: &GlobeViewState,
    focus: GeoPoint,
    render_zoom: f32,
    selected_root: Option<&Path>,
) {
    // Use GEBCO bathymetry — same zoom/LOD approach as global coastline.
    let bathy_zoom = view.local_zoom.clamp(1.0, 8.0);
    let Some(bathy) = contour_asset::load_global_bathymetry(selected_root, bathy_zoom, painter.ctx().clone()) else {
        return;
    };

    let half_extent_deg = visual_half_extent_for_zoom(view.local_zoom);
    let km_per_deg_lat = 111.32f32;
    let km_per_deg_lon = km_per_deg_lat * focus.lat.to_radians().cos().abs().max(0.2);
    let extent_x_km = (half_extent_deg * km_per_deg_lon).max(1.0);
    let extent_y_km = (half_extent_deg * km_per_deg_lat).max(1.0);

    let margin = half_extent_deg * 1.5;
    let min_lat = focus.lat - margin;
    let max_lat = focus.lat + margin;
    let min_lon = focus.lon - margin;
    let max_lon = focus.lon + margin;

    let _ = render_zoom; // used by caller for LOD selection via bathy_zoom

    const BATHY_ELEV_OFFSET: f32 = -5.0; // project just below sea level

    for contour in bathy.iter() {
        let in_view = contour.points.iter().any(|p| {
            p.lat >= min_lat && p.lat <= max_lat && p.lon >= min_lon && p.lon <= max_lon
        });
        if !in_view {
            continue;
        }

        let depth_norm = (-contour.elevation_m / 11_000.0_f32).clamp(0.0, 1.0);
        let major = ((-contour.elevation_m.round() as i32) % 1_000) < 50;
        let base_a = if major { 0.50_f32 } else { 0.25_f32 };
        let a = (base_a * (0.4 + depth_norm * 0.6) * 255.0) as u8;
        let r = (18.0 * (1.0 - depth_norm * 0.8)) as u8;
        let g = (55.0 * (1.0 - depth_norm * 0.6)) as u8;
        let b = (130 + (60.0 * depth_norm) as u8).min(255);
        let color = egui::Color32::from_rgba_premultiplied(r, g, b, a);
        let width = if major { 1.2 } else { 0.7 };
        let stroke = egui::Stroke::new(width, color);

        let points: Vec<_> = contour.points.iter()
            .filter_map(|p| {
                project_local(layout, view, focus, *p, BATHY_ELEV_OFFSET, extent_x_km, extent_y_km)
                    .map(|pp| pp.pos)
            })
            .collect();

        if points.len() >= 2 {
            painter.add(egui::Shape::line(points, stroke));
        }
    }
}

/// Draw global coastlines projected into the local oblique view.
/// Filters to only the polyline segments that overlap the current viewport.
fn draw_coastlines_local(
    painter: &egui::Painter,
    layout: &LocalLayout,
    view: &GlobeViewState,
    focus: GeoPoint,
    _render_zoom: f32,
    selected_root: Option<&Path>,
) {
    let half_extent_deg = visual_half_extent_for_zoom(view.local_zoom);
    let km_per_deg_lat = 111.32f32;
    let km_per_deg_lon = km_per_deg_lat * focus.lat.to_radians().cos().abs().max(0.2);
    let extent_x_km = (half_extent_deg * km_per_deg_lon).max(1.0);
    let extent_y_km = (half_extent_deg * km_per_deg_lat).max(1.0);

    let margin = half_extent_deg * 1.5;
    let min_lat = focus.lat - margin;
    let max_lat = focus.lat + margin;
    let min_lon = focus.lon - margin;
    let max_lon = focus.lon + margin;

    // GEBCO-derived global coastline (450m resolution).
    // Single LOD in load_global_coastlines so this never reloads on zoom change.
    let Some(coastlines) = contour_asset::load_global_coastlines(selected_root, 1.0, painter.ctx().clone()) else {
        return;
    };

    // Single thin white line — same visual weight as the topo contours.
    const COAST_ELEV: f32 = -3.0;
    let stroke = egui::Stroke::new(1.0, egui::Color32::from_rgba_premultiplied(220, 230, 255, 55));

    for coast in coastlines.iter() {
        let in_view = coast.points.iter().any(|p| {
            p.lat >= min_lat && p.lat <= max_lat && p.lon >= min_lon && p.lon <= max_lon
        });
        if !in_view { continue; }
        let points: Vec<_> = coast.points.iter()
            .filter_map(|p| {
                project_local(layout, view, focus, *p, COAST_ELEV, extent_x_km, extent_y_km)
                    .map(|pp| pp.pos)
            })
            .collect();
        if points.len() >= 2 {
            painter.add(egui::Shape::line(points, stroke));
        }
    }
}

fn draw_empty_state(painter: &egui::Painter, rect: egui::Rect, label: &str) {
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::proportional(18.0),
        theme::text_muted(),
    );
}

fn marker_elevation_m(selected_root: Option<&Path>, point: GeoPoint) -> f32 {
    let terrain_elevation_m = srtm_stream::sample_elevation_m(selected_root, point).unwrap_or(0.0);
    terrain_elevation_m + 18.0
}

fn local_geo_bounds(center: GeoPoint, view_zoom: f32) -> OsmGeoBounds {
    let half_extent_deg = visual_half_extent_for_zoom(view_zoom);
    OsmGeoBounds {
        min_lat: (center.lat - half_extent_deg).clamp(-85.0511, 85.0511),
        max_lat: (center.lat + half_extent_deg).clamp(-85.0511, 85.0511),
        min_lon: (center.lon - half_extent_deg).clamp(-180.0, 180.0),
        max_lon: (center.lon + half_extent_deg).clamp(-180.0, 180.0),
    }
}

fn road_tile_zoom(render_zoom: f32) -> u8 {
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

fn project_local(
    layout: &LocalLayout,
    view: &GlobeViewState,
    focus: GeoPoint,
    point: GeoPoint,
    elevation_m: f32,
    extent_x_km: f32,
    extent_y_km: f32,
) -> Option<ProjectedLocalPoint> {
    let x_km = (point.lon - focus.lon) * 111.32 * focus.lat.to_radians().cos().abs().max(0.2);
    // Standard orientation: positive y_km = north.  North is mapped upward on screen by
    // negating the ground_y_pitch / ground_z_pitch terms in the screen-y formula below.
    let y_km = (point.lat - focus.lat) * 111.32;

    let x = x_km / extent_x_km;
    let y = y_km / extent_y_km;
    // Normalize elevation against the current terrain span so vertical relief
    // scales with zoom instead of being added as a fixed screen-space offset.
    // Without this, horizontal distances expand/contract with zoom while
    // elevation stays effectively constant in pixels, which makes mountains
    // look wildly taller or flatter depending on zoom.
    let reference_span_km = ((extent_x_km + extent_y_km) * 0.5).max(1.0);
    let z = (elevation_m / 1000.0) * BASE_VERTICAL_EXAGGERATION / reference_span_km;

    let yaw_cos = view.local_yaw.cos();
    let yaw_sin = view.local_yaw.sin();
    let x_yaw = x * yaw_cos - y * yaw_sin;
    let y_yaw = x * yaw_sin + y * yaw_cos;

    let pitch_cos = view.local_pitch.cos();
    let pitch_sin = view.local_pitch.sin();
    let ground_y_pitch = y_yaw * pitch_cos;
    let ground_z_pitch = y_yaw * pitch_sin;
    let elevation_y_offset = z * pitch_sin;
    let elevation_z_offset = z * pitch_cos;
    let z_pitch = ground_z_pitch + elevation_z_offset;

    let ground_pitch_scale = layout.height * 0.55;
    // Keep in sync with shader gds constant: must be < gps/tan(max_pitch).
    // max_pitch=1.55 rad → tan≈48 → threshold ≈0.0114.  Using 0.01 for safety.
    let ground_depth_scale = layout.height * 0.01;
    let elevation_pitch_scale = layout.height * 0.55 * view.local_layer_spread;
    let elevation_depth_scale = layout.height * 0.24 * view.local_layer_spread;

    let pos = egui::pos2(
        layout.focus_center.x + x_yaw * layout.horizontal_scale,
        // Negate the ground terms so that positive y_yaw (north) moves upward on screen.
        // Elevation terms are unchanged: positive elevation still lifts features upward.
        layout.focus_center.y - ground_y_pitch * ground_pitch_scale
            + ground_z_pitch * ground_depth_scale
            - elevation_y_offset * elevation_pitch_scale
            - elevation_z_offset * elevation_depth_scale,
    );

    // Let egui's painter clip rect cull off-screen geometry; only reject points
    // that are wildly out of range (NaN / extreme float blown projections).
    (pos.x.is_finite()
        && pos.y.is_finite()
        && pos.x >= layout.center.x - layout.width * 4.0
        && pos.x <= layout.center.x + layout.width * 4.0
        && pos.y >= layout.center.y - layout.height * 4.0
        && pos.y <= layout.center.y + layout.height * 4.0)
        .then_some(ProjectedLocalPoint {
            pos,
            depth: (1.0 + z_pitch).clamp(0.0, 1.0),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_projection_expands_paris_contour_stack() {
        let model = AppModel::seed_demo();
        let event = model.selected_event().expect("selected event");
        let render_zoom = 6.0;
        let Some(contours) = (0..20).find_map(|_| {
            let contours = contour_asset::load_srtm_for_focus(
                model.selected_root.as_deref(),
                event.location,
                render_zoom,
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
                project_local(
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
