use crate::model::{AppModel, EventRecord, GeoPoint, GlobeViewState, NearbyCamera};
use crate::osm_ingest::{self, GeoBounds as OsmGeoBounds, RoadLayerKind};
use crate::terrain_assets;
use crate::theme;
use std::path::Path;

use super::contour_asset;
use super::globe_scene::GlobeScene;
use super::srtm_focus_cache;
use super::srtm_stream;

pub const LOCAL_TRANSITION_START_ZOOM: f32 = 4.0;
pub const LOCAL_MODE_MIN_ZOOM: f32 = 25.0;
const LOCAL_STREAM_RADIUS: i32 = 2;

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
    draw_frame(painter, rect);

    let layout = layout(rect);
    let Some(focus) = model.terrain_focus_location() else {
        draw_empty_state(painter, rect, "No terrain focus selected");
        return GlobeScene {
            event_markers: Vec::new(),
            camera_markers: Vec::new(),
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
    let (event_markers, camera_markers) = if !contours_slice.is_empty() {
        if let Some(event) = model.selected_event() {
            draw_markers(
                painter,
                &layout,
                &model.globe_view,
                model.selected_root.as_deref(),
                viewport_center,
                render_zoom,
                event,
                &nearby,
                model.selected_event_id.as_deref(),
                model.selected_camera_id.as_deref(),
                time,
            )
        } else {
            (Vec::new(), Vec::new())
        }
    } else {
        (Vec::new(), Vec::new())
    };
    draw_camera_links(
        painter,
        event_markers.first().map(|(_, pos)| *pos),
        &camera_markers,
    );
    draw_legend(painter, rect, "LOCAL EVENT TERRAIN", render_zoom);
    if let Some(status) = cache_status {
        draw_cache_progress(painter, rect, status);
    }

    GlobeScene {
        event_markers,
        camera_markers,
        beam_elevation_m: Some(beam_elevation_m),
    }
}

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
    let half_extent = srtm_focus_cache::half_extent_for_zoom(render_zoom);
    let bucket_step = half_extent * 0.45;
    let visual_half = visual_half_extent_for_zoom(view.local_zoom);
    let km_per_deg_lat = 111.32f32;
    let km_per_deg_lon = km_per_deg_lat * viewport_center.lat.to_radians().cos().abs().max(0.2);
    let extent_x_km = (visual_half * km_per_deg_lon).max(1.0);
    let extent_y_km = (visual_half * km_per_deg_lat).max(1.0);

    let center_lat_b = (viewport_center.lat / bucket_step).round() as i32;
    let center_lon_b = (viewport_center.lon / bucket_step).round() as i32;

    // Each tile's rendered footprint extends `half_extent` in every direction from
    // its bucket centre (see srtm_focus_cache::ensure_bucket_asset / GeoBounds::around).
    // bucket_step is the *spacing* between centres (half_extent * 0.45), so tiles
    // heavily overlap — but the correct polygon size is `half_extent`, not `bucket_step/2`.
    let half = half_extent;
    // Muted teal — clearly visible but not garish
    let fill_rgb = (18u8, 75u8, 90u8);
    let edge_rgb = (40u8, 140u8, 165u8);

    for dlat in -radius..=radius {
        for dlon in -radius..=radius {
            let lat_b = center_lat_b + dlat;
            let lon_b = center_lon_b + dlon;

            // Skip tiles that are already built — their contour lines will render on top.
            if ready_buckets.contains(&(lat_b, lon_b)) {
                continue;
            }

            let tile_lat = (lat_b as f32 * bucket_step).clamp(-89.9, 89.9);
            let tile_lon = lon_b as f32 * bucket_step;

            // Project the four corners of this tile footprint at ground level.
            // Order: NW → NE → SE → SW (clockwise in screen space) so egui's
            // convex_polygon winding is satisfied under the oblique projection.
            let geo_corners = [
                GeoPoint {
                    lat: tile_lat + half,
                    lon: tile_lon - half,
                },
                GeoPoint {
                    lat: tile_lat + half,
                    lon: tile_lon + half,
                },
                GeoPoint {
                    lat: tile_lat - half,
                    lon: tile_lon + half,
                },
                GeoPoint {
                    lat: tile_lat - half,
                    lon: tile_lon - half,
                },
            ];
            let screen_corners: Vec<egui::Pos2> = geo_corners
                .iter()
                .filter_map(|&c| {
                    project_local(
                        layout,
                        view,
                        viewport_center,
                        c,
                        0.0,
                        extent_x_km,
                        extent_y_km,
                    )
                })
                .map(|p| p.pos)
                .collect();

            if screen_corners.len() < 4 {
                continue;
            }

            // Slow collective throb: ~4 s cycle, all cells in phase.
            // sin ∈ [-1, 1] → remap to [0, 1].  Keep a floor so it never
            // fully disappears; keep a ceiling so it stays subtle.
            let phase =
                ((time as f32 * std::f32::consts::TAU / 4.0).sin() * 0.5 + 0.5).clamp(0.0, 1.0);
            let breath = 0.20 + phase * 0.65; // 0.20 … 0.85

            painter.add(egui::Shape::convex_polygon(
                screen_corners,
                egui::Color32::from_rgba_premultiplied(
                    fill_rgb.0,
                    fill_rgb.1,
                    fill_rgb.2,
                    (breath * 28.0) as u8, // 6 … 24
                ),
                egui::Stroke::new(
                    1.0,
                    egui::Color32::from_rgba_premultiplied(
                        edge_rgb.0,
                        edge_rgb.1,
                        edge_rgb.2,
                        (breath * 110.0) as u8, // 22 … 94
                    ),
                ),
            ));
        }
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
                egui::Color32::from_rgb(244, 123, 61)
            } else {
                egui::Color32::from_rgb(121, 212, 236)
            }
            .gamma_multiply((if major { 1.0 } else { 0.78 }) * alpha),
        );

        painter.add(egui::Shape::line(points, stroke));
    }
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
        return;
    }

    let bounds = local_geo_bounds(viewport_center, view.local_zoom);
    let tile_zoom = road_tile_zoom(render_zoom);
    let half_extent_deg = visual_half_extent_for_zoom(view.local_zoom);
    let km_per_deg_lat = 111.32f32;
    let km_per_deg_lon = km_per_deg_lat * viewport_center.lat.to_radians().cos().abs().max(0.2);
    let extent_x_km = (half_extent_deg * km_per_deg_lon).max(1.0);
    let extent_y_km = (half_extent_deg * km_per_deg_lat).max(1.0);

    if show_minor_roads {
        let roads = osm_ingest::load_roads_for_bounds(
            selected_root,
            bounds,
            tile_zoom,
            RoadLayerKind::Minor,
        );
        draw_road_layer(
            painter,
            layout,
            view,
            selected_root,
            viewport_center,
            extent_x_km,
            extent_y_km,
            &roads,
            egui::Stroke::new(0.8, egui::Color32::from_rgb(116, 132, 142)),
        );
    }

    if show_major_roads {
        let roads = osm_ingest::load_roads_for_bounds(
            selected_root,
            bounds,
            tile_zoom,
            RoadLayerKind::Major,
        );
        draw_road_layer(
            painter,
            layout,
            view,
            selected_root,
            viewport_center,
            extent_x_km,
            extent_y_km,
            &roads,
            egui::Stroke::new(1.35, egui::Color32::from_rgb(255, 210, 92)),
        );
    }
}

fn draw_road_layer(
    painter: &egui::Painter,
    layout: &LocalLayout,
    view: &GlobeViewState,
    selected_root: Option<&Path>,
    viewport_center: GeoPoint,
    extent_x_km: f32,
    extent_y_km: f32,
    roads: &[osm_ingest::RoadPolyline],
    stroke: egui::Stroke,
) {
    for road in roads {
        let points: Vec<_> = road
            .points
            .iter()
            .filter_map(|point| {
                let elevation_m =
                    srtm_stream::sample_elevation_m(selected_root, *point).unwrap_or(0.0) + 3.0;
                project_local(
                    layout,
                    view,
                    viewport_center,
                    *point,
                    elevation_m,
                    extent_x_km,
                    extent_y_km,
                )
                .map(|projected| projected.pos)
            })
            .collect();

        if points.len() >= 2 {
            painter.add(egui::Shape::line(points, stroke));
        }
    }
}

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

    let event_marker = project_local(
        layout,
        view,
        viewport_center,
        event.location,
        marker_elevation_m(selected_root, event.location),
        extent_x_km,
        extent_y_km,
    );
    if let Some(event_marker) = event_marker {
        draw_event_marker(
            painter,
            event_marker,
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

fn draw_event_marker(
    painter: &egui::Painter,
    marker: ProjectedLocalPoint,
    event: &EventRecord,
    is_selected: bool,
    time: f64,
) {
    let radius = 5.1 + marker.depth * 1.8;
    if is_selected {
        let pulse = radius + 4.0 + ((time as f32 * 2.5).sin() + 1.0) * 2.4;
        painter.circle_stroke(
            marker.pos,
            pulse,
            egui::Stroke::new(
                1.3,
                egui::Color32::from_rgba_premultiplied(255, 241, 212, 170),
            ),
        );
    }

    painter.circle_filled(marker.pos, radius, event.severity.color());
    painter.circle_stroke(
        marker.pos,
        radius + 2.1,
        egui::Stroke::new(1.0, theme::hot_color().gamma_multiply(0.8)),
    );
}

fn draw_camera_marker(painter: &egui::Painter, marker: ProjectedLocalPoint, is_selected: bool) {
    let radius = 3.4 + marker.depth;
    let color = if is_selected {
        egui::Color32::from_rgb(215, 245, 252)
    } else {
        theme::camera_color()
    };

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

fn draw_cache_progress(
    painter: &egui::Painter,
    rect: egui::Rect,
    status: srtm_focus_cache::FocusContourRegionStatus,
) {
    if status.total_assets == 0 || status.ready_assets >= status.total_assets {
        return;
    }

    let frame_rect = egui::Rect::from_min_size(
        egui::pos2(rect.right() - 232.0, rect.bottom() - 88.0),
        egui::vec2(184.0, 36.0),
    );
    let bar_rect = egui::Rect::from_min_size(
        frame_rect.left_bottom() + egui::vec2(0.0, -12.0),
        egui::vec2(frame_rect.width(), 8.0),
    );
    let progress = (status.ready_assets as f32 / status.total_assets as f32).clamp(0.0, 1.0);

    painter.rect_filled(
        frame_rect,
        6.0,
        egui::Color32::from_rgba_premultiplied(7, 18, 24, 208),
    );
    painter.rect_stroke(
        frame_rect,
        6.0,
        egui::Stroke::new(1.0, egui::Color32::from_rgb(24, 63, 79)),
        egui::StrokeKind::Outside,
    );
    painter.text(
        frame_rect.left_top() + egui::vec2(8.0, 6.0),
        egui::Align2::LEFT_TOP,
        format!(
            "CACHE {} / {}  ·  {} PENDING",
            status.ready_assets, status.total_assets, status.pending_assets
        ),
        egui::FontId::monospace(11.0),
        theme::text_muted(),
    );
    painter.rect_filled(
        bar_rect,
        4.0,
        egui::Color32::from_rgba_premultiplied(15, 40, 49, 230),
    );
    if progress > 0.0 {
        let fill_rect = egui::Rect::from_min_max(
            bar_rect.min,
            egui::pos2(
                bar_rect.left() + bar_rect.width() * progress,
                bar_rect.bottom(),
            ),
        );
        painter.rect_filled(fill_rect, 4.0, theme::topo_color());
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
    let z = elevation_m / 1000.0;

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

    let pos = egui::pos2(
        layout.focus_center.x + x_yaw * layout.horizontal_scale,
        // Negate the ground terms so that positive y_yaw (north) moves upward on screen.
        // Elevation terms are unchanged: positive elevation still lifts features upward.
        layout.focus_center.y - ground_y_pitch * layout.height * 0.55 + ground_z_pitch * 48.0
            - elevation_y_offset * view.local_layer_spread * 56.0
            - elevation_z_offset * view.local_layer_spread * 24.0,
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
