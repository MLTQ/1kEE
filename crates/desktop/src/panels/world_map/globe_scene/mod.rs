use crate::arcgis_source;
use crate::model::{AppModel, ArcGisFeature, GeoPoint, GlobeViewState};
use crate::theme;

use super::camera::{self, GlobeLod};
use super::contour_asset;
use super::gebco_depth_fill;
use super::globe_pass;
use super::srtm_focus_cache;
use super::terrain_field;

mod geography;
mod markers;
mod projection;

pub use projection::project_geo;

pub struct GlobeScene {
    pub event_markers: Vec<(String, egui::Pos2)>,
    pub camera_markers: Vec<(String, egui::Pos2)>,
    /// MMSI → screen position for click/hover detection.
    pub ship_markers: Vec<(u64, egui::Pos2)>,
    /// ICAO24 → screen position for hover detection.
    pub flight_markers: Vec<(String, egui::Pos2)>,
    /// (source_url, object_id, screen_pos) for ArcGIS feature click detection.
    pub arcgis_feature_markers: Vec<(String, i64, egui::Pos2)>,
    /// Terrain elevation (metres) at the beam contact point, if available.
    pub beam_elevation_m: Option<f32>,
}

pub struct GlobeLayout {
    pub center: egui::Pos2,
    pub radius: f32,
    pub focal_length: f32,
    pub camera_distance: f32,
}

#[derive(Clone, Copy)]
pub struct ProjectedPoint {
    pub pos: egui::Pos2,
    pub depth: f32,
    pub front_facing: bool,
}

pub fn paint(painter: &egui::Painter, rect: egui::Rect, model: &AppModel, time: f64) -> GlobeScene {
    painter.rect_filled(rect, 12.0, theme::canvas_background());

    let lod = camera::lod(&model.globe_view);
    let layout = globe_layout(rect, &model.globe_view);
    let selected_root = model.selected_root.as_deref();

    // ── GPU globe backdrop (terrain shading) ──────────────────────────────
    // Graticule is drawn CPU-side after all terrain layers (see below) so it
    // renders on top of bathymetry/coastlines and is pixel-crisp rather than
    // the soft SDF approximation the fragment shader produces.
    let ppp = painter.ctx().pixels_per_point();
    let show_grat = model.show_graticule && !model.globe_view.local_mode;
    painter.add(
        globe_pass::GlobeCallback::new(
            layout.center,
            layout.radius,
            layout.focal_length,
            layout.camera_distance,
            model.globe_view.yaw,
            model.globe_view.pitch,
            ppp,
            false, // graticule always off in GPU pass — drawn CPU-side instead
            model.moon_mode,
            theme::scene_backdrop(),
            theme::topo_color(),
            theme::wireframe_color(),
            theme::grid_color(),
            theme::hot_color(),
        )
        .into_paint_callback(rect),
    );

    // Outer panel rect stroke (was part of draw_backdrop)
    painter.rect_stroke(
        rect.shrink(6.0),
        12.0,
        egui::Stroke::new(0.7, theme::topo_color().gamma_multiply(0.45)),
        egui::StrokeKind::Outside,
    );

    if model.show_reticle {
        draw_hud_frame(painter, rect);
    }

    if !model.moon_mode {
        if model.show_bathymetry {
            geography::draw_global_bathymetry(painter, &layout, &model.globe_view, selected_root);
        }
        if model.show_coastlines {
            geography::draw_global_coastlines(painter, &layout, &model.globe_view, selected_root);
        }
        geography::draw_global_topo(painter, &layout, &model.globe_view, selected_root);
        geography::draw_srtm_on_globe(painter, &layout, &model.globe_view, &lod, selected_root);
    } else {
        geography::draw_lunar_topo(painter, &layout, &model.globe_view, selected_root);
    }

    // ── GeoJSON user overlay layers ────────────────────────────────────────
    if !model.geojson_layers.is_empty() {
        geography::draw_geojson_layers(painter, &layout, &model.globe_view, &model.geojson_layers);
    }

    if model.show_reticle {
        draw_zoom_crosshair(painter, &layout, &model.globe_view, time);
    }

    // ── CPU graticule — drawn after all terrain/bathy layers so it sits on top ──
    if show_grat {
        draw_graticule(painter, &layout, &model.globe_view);
    }

    // ── Stellar correspondence layer ───────────────────────────────────────────
    if model.show_stellar_correspondence && !model.globe_view.local_mode {
        super::stellar_layer::draw_stellar_correspondence(
            painter,
            &layout,
            &model.globe_view,
            model.stellar_jd,
            model.stellar_precess,
        );
        if model.show_planet_trails {
            super::stellar_layer::draw_planet_trails(
                painter,
                &layout,
                &model.globe_view,
                model.stellar_jd,
                model.planet_trail_years,
            );
        }
        if model.show_planets {
            super::stellar_layer::draw_planets(
                painter,
                &layout,
                &model.globe_view,
                model.stellar_jd,
            );
        }
    }

    let selected_event_id = model.selected_event_id.as_deref();
    let selected_camera_id = model.selected_camera_id.as_deref();
    let nearby = model.nearby_cameras(250.0);

    // ── Replay flares (shown instead of live markers while replay is active) ──
    if model.replay_mode && !model.moon_mode {
        if let Some(state) = &model.replay_state {
            let wall_elapsed = state.wall_elapsed();
            for flare in &state.active_flares {
                let Some(base) = projection::project_geo(
                    &layout,
                    &model.globe_view,
                    flare.event.location,
                    lod.altitude_scale * 0.7,
                ) else {
                    continue;
                };
                if !base.front_facing {
                    continue;
                }
                let extra_r = (135.0 / layout.radius).clamp(0.060, 0.220);
                let tip = projection::project_geo_elevated(
                    &layout,
                    &model.globe_view,
                    flare.event.location,
                    lod.altitude_scale * 0.7,
                    extra_r,
                )
                .map(|p| p.pos)
                .unwrap_or(base.pos);
                markers::draw_replay_flare(painter, base, tip, flare, wall_elapsed);
            }
        }
    }

    let event_markers: Vec<_> = if !model.show_event_markers || model.replay_mode || model.moon_mode
    {
        Vec::new()
    } else {
        model
            .events
            .iter()
            .filter_map(|event| {
                let base = projection::project_geo(
                    &layout,
                    &model.globe_view,
                    event.location,
                    lod.altitude_scale * 0.7,
                )?;
                // Beam tip: project the same geographic point at a higher
                // radius so that 3-D perspective foreshortening is correct.
                // When the event faces the camera, base and tip project to
                // almost the same screen position (tiny beam).  When the event
                // is on the limb, the tip projects far from the base (full
                // beam).  This eliminates the "spinning" artefact caused by
                // computing the direction in screen space.
                let extra_r = (135.0 / layout.radius).clamp(0.060, 0.220);
                let tip = projection::project_geo_elevated(
                    &layout,
                    &model.globe_view,
                    event.location,
                    lod.altitude_scale * 0.7,
                    extra_r,
                )
                .map(|p| p.pos)
                .unwrap_or(base.pos); // fallback: zero-length beam
                markers::draw_event_marker(
                    painter,
                    base,
                    tip,
                    event,
                    selected_event_id == Some(event.id.as_str()),
                    time,
                );
                Some((event.id.clone(), base.pos))
            })
            .collect()
    };

    let camera_markers: Vec<_> = if model.moon_mode {
        Vec::new()
    } else {
        nearby
            .iter()
            .filter_map(|camera| {
                projection::project_geo(
                    &layout,
                    &model.globe_view,
                    camera.location,
                    lod.altitude_scale * 0.35,
                )
                .map(|projected| {
                    let is_selected = selected_camera_id == Some(camera.id.as_str());
                    markers::draw_camera_marker(painter, projected, is_selected);
                    (camera.id.clone(), projected.pos)
                })
            })
            .collect()
    };

    // ── AIS ship markers ──────────────────────────────────────────────────
    // `model.tracks` is refreshed each frame by `render_world_map` before
    // this function is called.  We just draw whatever is cached.
    if model.show_ships && !model.globe_view.local_mode {
        markers::draw_ships(
            painter,
            &layout,
            &model.globe_view,
            &model.tracks,
            model.selected_track_mmsi,
        );
    }

    // ── ADS-B flight markers ───────────────────────────────────────────────
    if model.show_flights && !model.globe_view.local_mode {
        markers::draw_flights(
            painter,
            &layout,
            &model.globe_view,
            &model.flights,
            model.selected_flight_icao24.as_deref(),
        );
    }

    // ── ArcGIS feature markers ─────────────────────────────────────────────
    let arcgis_feature_markers =
        if !model.globe_view.local_mode && !model.arcgis_features.is_empty() {
            draw_arcgis_features(
                painter,
                &layout,
                &model.globe_view,
                &model.arcgis_features,
                model.selected_arcgis_feature.as_ref(),
            )
        } else {
            Vec::new()
        };

    let ship_markers: Vec<(u64, egui::Pos2)> = if model.show_ships && !model.globe_view.local_mode {
        model
            .tracks
            .iter()
            .filter_map(|t| {
                projection::project_geo(&layout, &model.globe_view, t.location, 0.0)
                    .filter(|p| p.front_facing)
                    .map(|p| (t.mmsi, p.pos))
            })
            .collect()
    } else {
        Vec::new()
    };

    if let Some((_, event_marker)) = event_markers
        .iter()
        .find(|(event_id, _)| selected_event_id == Some(event_id.as_str()))
    {
        markers::draw_camera_links(painter, *event_marker, &camera_markers);
    }
    draw_legend(painter, rect, &layout, &model.globe_view, &lod);

    // ── Lunar contour build progress ────────────────────────────────────────
    if model.moon_mode {
        let (lunar_ready, lunar_building, lunar_total) = srtm_focus_cache::lunar_tile_counts(
            selected_root,
            model.globe_view.local_center,
            1.5, // fixed globe tile zoom (mirrors GLOBE_TILE_ZOOM in contour_asset)
            2,   // radius
        );
        if lunar_total > 0 && lunar_ready < lunar_total {
            draw_lunar_build_overlay(
                painter,
                rect,
                &layout,
                &model.globe_view,
                lunar_ready,
                lunar_building,
                lunar_total,
                time,
            );
        }
    }

    let flight_markers: Vec<(String, egui::Pos2)> =
        if model.show_flights && !model.globe_view.local_mode {
            model
                .flights
                .iter()
                .filter_map(|f| {
                    projection::project_geo(&layout, &model.globe_view, f.location, 0.0)
                        .filter(|p| p.front_facing)
                        .map(|p| (f.icao24.clone(), p.pos))
                })
                .collect()
        } else {
            Vec::new()
        };

    GlobeScene {
        event_markers,
        camera_markers,
        ship_markers,
        flight_markers,
        arcgis_feature_markers,
        beam_elevation_m: None,
    }
}

/// Draw ArcGIS feature markers as filled circles with glow halos.
/// Color is per-layer. Features with casualties get a larger outer ring.
/// Returns a list of (source_url, object_id, screen_pos) for click detection.
fn draw_arcgis_features(
    painter: &egui::Painter,
    layout: &GlobeLayout,
    view: &GlobeViewState,
    features: &[ArcGisFeature],
    selected: Option<&(String, i64)>,
) -> Vec<(String, i64, egui::Pos2)> {
    let mut markers_out = Vec::new();
    for feat in features {
        let Some(proj) = projection::project_geo(layout, view, feat.location, 0.0) else {
            continue;
        };
        if !proj.front_facing {
            continue;
        }

        let col = arcgis_source::feature_color(feat);
        let pos = proj.pos;
        let has_cas = feat.has_casualties();

        // White selection ring around the selected feature
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

        // Outer glow halo
        painter.circle_stroke(
            pos,
            if has_cas { 9.0 } else { 6.5 },
            egui::Stroke::new(2.5, col.gamma_multiply(0.12)),
        );
        if has_cas {
            painter.circle_stroke(pos, 6.5, egui::Stroke::new(1.5, col.gamma_multiply(0.22)));
        }

        // Filled core dot
        painter.circle_filled(pos, if has_cas { 3.5 } else { 2.5 }, col);
        // Bright centre spot
        painter.circle_filled(pos, 1.2, col.gamma_multiply(1.4));

        markers_out.push((feat.source_url.clone(), feat.object_id, pos));
    }
    markers_out
}

pub fn globe_layout(rect: egui::Rect, view: &GlobeViewState) -> GlobeLayout {
    // zoom_t: 0 at zoom=0.6, 1 at zoom=50.  Logarithmic so each scroll notch
    // gives equal perceived zoom step.
    let zoom_t = ((view.zoom.ln() - 0.6f32.ln()) / (50.0f32.ln() - 0.6f32.ln())).clamp(0.0, 1.0);
    let base_radius = (rect.width() * 0.21).min(rect.height() * 0.30);
    // At zoom_t=0 the globe is a small sphere; at zoom_t=1 it is 9× larger,
    // filling and greatly exceeding the viewport so only a country-scale
    // surface patch is visible.
    let radius = base_radius * (1.0 + zoom_t * 8.0);
    GlobeLayout {
        center: egui::pos2(
            rect.center().x + rect.width() * 0.04,
            rect.center().y + rect.height() * 0.01,
        ),
        radius,
        // Narrower FOV (higher focal_length) as we zoom in for a flatter,
        // more map-like perspective at high zoom.
        focal_length: 2.05 + zoom_t * 1.0,
        // Camera moves closer to the sphere surface at high zoom.
        // Keep at least 2.0 so the front pole stays visible (depth > 0).
        camera_distance: 3.15 - zoom_t * 1.15,
    }
}

/// Convert a screen-space position to geographic coordinates (lat/lon degrees)
/// by intersecting a perspective ray with the unit sphere.
/// Returns `None` if the cursor is not over the globe (ray misses the sphere).
pub fn screen_to_latlon(
    rect: egui::Rect,
    view: &GlobeViewState,
    screen_pos: egui::Pos2,
) -> Option<GeoPoint> {
    let layout = globe_layout(rect, view);

    let dx = (layout.center.x - screen_pos.x) / layout.radius;
    let dy = (layout.center.y - screen_pos.y) / layout.radius;
    let fl = layout.focal_length;
    let cd = layout.camera_distance;

    // Quadratic: t^2*(dx^2+dy^2+fl^2) - 2*t*cd*fl + (cd^2-1) = 0
    let a = dx * dx + dy * dy + fl * fl;
    let b = -2.0 * cd * fl;
    let c = cd * cd - 1.0;
    let disc = b * b - 4.0 * a * c;
    if disc < 0.0 {
        return None; // cursor is off the globe
    }
    let t = (-b - disc.sqrt()) / (2.0 * a); // smaller root = front face

    // Point on sphere in rotated frame
    let px = t * dx;
    let py = t * dy;
    let pz = cd - t * fl;

    // Inverse pitch rotation
    let pitch = view.pitch;
    let y2 = py * pitch.cos() + pz * pitch.sin();
    let z2 = -py * pitch.sin() + pz * pitch.cos();

    // Inverse yaw rotation
    let yaw = view.yaw;
    let x3 = px * yaw.cos() - z2 * yaw.sin();
    let z3 = px * yaw.sin() + z2 * yaw.cos();

    let lat_rad = y2.clamp(-1.0, 1.0).asin();
    let lon_rad = z3.atan2(x3);

    Some(GeoPoint {
        lat: lat_rad.to_degrees(),
        lon: lon_rad.to_degrees(),
    })
}

fn draw_hud_frame(painter: &egui::Painter, rect: egui::Rect) {
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

/// Red glowing crosshair pinned to `view.local_center` — the point the camera
/// is centred on and will zoom into when transitioning to local terrain mode.
/// Provides spatial context for where you are on the globe surface.
fn draw_zoom_crosshair(
    painter: &egui::Painter,
    layout: &GlobeLayout,
    view: &GlobeViewState,
    time: f64,
) {
    // Only visible once the user has zoomed in enough to care about local terrain
    if view.zoom < 1.5 {
        return;
    }
    let alpha = ((view.zoom - 1.5) / 1.5).clamp(0.0, 1.0);

    let Some(projected) = projection::project_geo(layout, view, view.local_center, 0.025) else {
        return;
    };

    // Cherry red — distinct from the orange "hot" palette used elsewhere
    let cherry = egui::Color32::from_rgb(210, 18, 50);
    let pos = projected.pos;
    let ring_r: f32 = 9.0;
    let gap: f32 = 3.5;
    let arm_len: f32 = 8.0;

    // Outer pulsing bloom ring
    let pulse = (time as f32 * 1.8).sin() * 0.5 + 0.5;
    let bloom_r = ring_r + 5.0 + pulse * 3.5;
    painter.circle_stroke(
        pos,
        bloom_r,
        egui::Stroke::new(
            6.0,
            cherry.gamma_multiply(alpha * 0.07 * (0.6 + pulse * 0.4)),
        ),
    );

    // Secondary soft halo
    painter.circle_stroke(
        pos,
        ring_r + 3.0,
        egui::Stroke::new(3.5, cherry.gamma_multiply(alpha * 0.18)),
    );

    // Crisp main ring
    painter.circle_stroke(
        pos,
        ring_r,
        egui::Stroke::new(1.3, cherry.gamma_multiply(alpha * 0.92)),
    );

    // Centre dot
    painter.circle_filled(pos, 2.0, cherry.gamma_multiply(alpha));

    // Four tick arms extending outward from the ring with a small gap
    let inner = ring_r + gap;
    let outer = ring_r + gap + arm_len;
    for &(dx, dy) in &[(1.0f32, 0.0f32), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
        painter.line_segment(
            [
                egui::pos2(pos.x + dx * inner, pos.y + dy * inner),
                egui::pos2(pos.x + dx * outer, pos.y + dy * outer),
            ],
            egui::Stroke::new(1.3, cherry.gamma_multiply(alpha * 0.85)),
        );
    }
}

fn draw_legend(
    painter: &egui::Painter,
    rect: egui::Rect,
    layout: &GlobeLayout,
    view: &GlobeViewState,
    lod: &GlobeLod,
) {
    painter.text(
        egui::pos2(rect.left() + 24.0, rect.bottom() - 86.0),
        egui::Align2::LEFT_TOP,
        format!(
            "TACTICAL GLOBE\n3D ORBIT {}\nZOOM {:.2}x | LOD {}",
            if view.auto_spin { "AUTO" } else { "MANUAL" },
            view.zoom,
            lod.contour_layers
        ),
        egui::FontId::monospace(12.0),
        theme::text_muted(),
    );

    painter.text(
        egui::pos2(
            layout.center.x + layout.radius + 52.0,
            layout.center.y - 22.0,
        ),
        egui::Align2::LEFT_TOP,
        "RANGE GATE\n250 KM",
        egui::FontId::monospace(11.0),
        theme::hot_color(),
    );
}

// ── CPU graticule — delegated to sibling module ───────────────────────────────

fn draw_graticule(painter: &egui::Painter, layout: &GlobeLayout, view: &GlobeViewState) {
    super::graticule::draw_graticule(painter, layout, view);
}

/// Progress card + pulsing globe ring shown while lunar contour tiles are building.
fn draw_lunar_build_overlay(
    painter: &egui::Painter,
    rect: egui::Rect,
    layout: &GlobeLayout,
    view: &GlobeViewState,
    ready: usize,
    building: usize,
    total: usize,
    time: f64,
) {
    // ── Progress card (bottom-right) ──────────────────────────────────────
    const CARD_W: f32 = 220.0;
    const CARD_H: f32 = 36.0;
    let frame = egui::Rect::from_min_size(
        egui::pos2(rect.right() - 12.0 - CARD_W, rect.bottom() - 12.0 - CARD_H),
        egui::vec2(CARD_W, CARD_H),
    );
    let bar = egui::Rect::from_min_size(
        frame.left_bottom() + egui::vec2(0.0, -10.0),
        egui::vec2(frame.width(), 6.0),
    );
    let progress = (ready as f32 / total as f32).clamp(0.0, 1.0);
    let accent = egui::Color32::from_rgb(155, 200, 248); // lunar blue-white

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
        format!("SLDEM {ready} / {total}  ·  {building} BUILDING"),
        egui::FontId::monospace(10.5),
        theme::text_muted(),
    );
    // Track
    painter.rect_filled(bar, 3.0, theme::panel_fill(230).gamma_multiply(2.5));
    // Fill
    if progress > 0.0 {
        let filled = egui::Rect::from_min_max(
            bar.min,
            egui::pos2(bar.left() + bar.width() * progress, bar.bottom()),
        );
        painter.rect_filled(filled, 3.0, accent);
    }

    // ── Pulsing ring centred on the reticle ──────────────────────────────
    // Project view.local_center (the reticle position) so the ring is
    // always concentric with the red crosshair, regardless of orbit angle.
    // Fall back to layout.center if the reticle is on the far hemisphere.
    let ring_center = projection::project_geo(layout, view, view.local_center, 0.0)
        .map(|p| p.pos)
        .unwrap_or(layout.center);
    let pulse = (time as f32 * 0.9).sin() * 0.5 + 0.5;
    let ring_alpha = (0.06 + pulse * 0.10) * (1.0 - progress * 0.6);
    let ring_r = layout.radius + 6.0 + pulse * 8.0;
    painter.circle_stroke(
        ring_center,
        ring_r,
        egui::Stroke::new(2.5 + pulse * 1.5, accent.gamma_multiply(ring_alpha)),
    );
}
