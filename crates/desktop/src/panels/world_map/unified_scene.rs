//! Single continuous planet-to-metre world map scene.
//!
//! Replaces the split `globe_scene` / `local_terrain_scene` modes with one
//! scale-driven renderer backed by [`MapProjection`] (`f64` ECEF → RTC `f32`)
//! and a shared Reversed-Z `Depth32Float` scene target. Every layer activates
//! and refines continuously by ground resolution (`meters_per_pixel` /
//! `visible_half_extent_deg`) in both **Perspective** and **Orthographic** modes.

use crate::arcgis_source;
use crate::model::{
    ActiveBody, AppModel, ArcGisFeature, GeoJsonGeometry, GeoJsonLayer, GeoPoint,
};
use crate::theme;
use glam::DVec3;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use super::contour_asset::{self, ContourPath};
use super::gebco_depth_fill;
use super::globe_scene::{self, GlobeScene, ProjectedPoint};
use super::local_terrain_scene::{self, deflock_layer};
use super::marker_style::{MapMarker, draw_camera_spire};
use super::projection::{MapProjection, geo_to_ecef};
use super::unified_globe_pass::UnifiedGlobeCallback;
use super::unified_line_pass::{RtcLineBatch, RtcSegmentInstance, UnifiedLineCallback};
use super::unified_scene_target::{BeginSceneCallback, CompositeSceneCallback};

/// Cache static `RtcLineBatch` conversions keyed by `(layer_tag, Arc::as_ptr, color_key)`
/// so static global/tile vector layers are never re-converted per frame.
struct RtcBatchCache {
    entries: HashMap<(u64, usize, u64), RtcLineBatch>,
}

fn rtc_batch_cache() -> &'static Mutex<RtcBatchCache> {
    static CACHE: OnceLock<Mutex<RtcBatchCache>> = OnceLock::new();
    CACHE.get_or_init(|| {
        Mutex::new(RtcBatchCache {
            entries: HashMap::new(),
        })
    })
}

fn cached_contour_batch(
    layer_tag: u64,
    contours: &Arc<Vec<ContourPath>>,
    color_key: u64,
    radius_m: f64,
    use_elevation: bool,
    stroke_minor_px: f32,
    stroke_major_px: f32,
    alpha: f32,
    extra_radial_m: f32,
    depth_bias_ndc: f32,
    color_fn: impl Fn(&ContourPath) -> (egui::Color32, bool),
) -> Option<RtcLineBatch> {
    if contours.is_empty() {
        return None;
    }
    let ptr = Arc::as_ptr(contours) as usize;
    let cache_key = (layer_tag, ptr, color_key);
    {
        let cache = rtc_batch_cache().lock().ok()?;
        if let Some(existing) = cache.entries.get(&cache_key) {
            let mut b = existing.clone();
            b.stroke_minor_px = stroke_minor_px;
            b.stroke_major_px = stroke_major_px;
            b.alpha = alpha;
            b.extra_radial_m = extra_radial_m;
            b.depth_bias_ndc = depth_bias_ndc;
            return Some(b);
        }
    }

    // Choose the first finite vertex as the tile/layer ECEF anchor.
    let origin = contours
        .iter()
        .flat_map(|c| c.points.iter())
        .find(|p| p.lat.is_finite() && p.lon.is_finite())
        .map(|p| geo_to_ecef(p.lat as f64, p.lon as f64, 0.0, radius_m))
        .unwrap_or(DVec3::X * radius_m);

    let mut segs = Vec::new();
    for contour in contours.iter() {
        let (col, major) = color_fn(contour);
        let elev = if use_elevation { contour.elevation_m } else { 0.0 };
        for pair in contour.points.windows(2) {
            if !pair[0].lat.is_finite()
                || !pair[0].lon.is_finite()
                || !pair[1].lat.is_finite()
                || !pair[1].lon.is_finite()
            {
                continue;
            }
            segs.push(RtcSegmentInstance::from_geo(
                pair[0], elev, pair[1], elev, origin, radius_m, col, major,
            ));
        }
    }
    if segs.is_empty() {
        return None;
    }

    let key = layer_tag ^ (ptr as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ color_key;
    let batch = RtcLineBatch {
        key,
        version: (ptr as u64) ^ color_key,
        origin_ecef: origin,
        segments: Arc::new(segs),
        stroke_minor_px,
        stroke_major_px,
        alpha,
        extra_radial_m,
        depth_bias_ndc,
        horizon_cull: true,
        palette_override: None,
    };

    if let Ok(mut cache) = rtc_batch_cache().lock() {
        if cache.entries.len() > 256 {
            cache.entries.clear();
        }
        cache.entries.insert(cache_key, batch.clone());
    }
    Some(batch)
}

pub fn paint(
    painter: &egui::Painter,
    rect: egui::Rect,
    model: &AppModel,
    time: f64,
) -> GlobeScene {
    let ppp = painter.ctx().pixels_per_point();
    let proj = MapProjection::from_view(&model.globe_view, model.active_body, rect, ppp);
    let half_deg = proj.visible_half_extent_deg();
    let atlas_earth =
        model.map_theme == theme::MapTheme::Atlas && model.active_body == ActiveBody::Earth;
    let selected_root = model.selected_root.as_deref();
    let stroke_px = model.contour_stroke_width_px(ppp);

    // 1. Canvas background fill
    painter.rect_filled(
        rect,
        if model.obs_view { 0.0 } else { 12.0 },
        theme::canvas_background(),
    );

    // 2. Clear shared Reversed-Z (color, depth) scene target + draw RTE planet backdrop
    painter.add(BeginSceneCallback { rect }.into_paint_callback());
    painter.add(
        UnifiedGlobeCallback::from_projection(
            &proj,
            theme::scene_backdrop(),
            theme::topo_color(),
            theme::wireframe_color(),
            theme::grid_color(),
            theme::hot_color(),
        )
        .into_paint_callback(),
    );

    // 3. Earth Atlas 3D displaced relief pyramid (Overview -> 5° GEBCO -> SRTM -> Fine DEM)
    if atlas_earth {
        super::atlas_relief::draw_unified(painter, &proj, model);
    }

    // 4. Collect all GPU RTC vector batches (contours, coastlines, bathymetry, roads)
    let mut line_batches: Vec<RtcLineBatch> = Vec::new();
    let radius_m = proj.radius_m;

    if model.active_body == ActiveBody::Earth {
        // Non-Atlas global coastlines & bathymetry isobaths
        if model.show_bathymetry && !atlas_earth {
            if let Some(bathy) = contour_asset::load_global_bathymetry(
                selected_root,
                model.globe_view.zoom,
                painter.ctx().clone(),
            ) && let Some(batch) = cached_contour_batch(
                0xBA74_0001,
                &bathy,
                1,
                radius_m,
                false,
                stroke_px * 0.85,
                stroke_px,
                0.92,
                0.0,
                0.0001,
                |c| {
                    let depth_norm = (-c.elevation_m / 11_000.0_f32).clamp(0.0, 1.0);
                    let major = ((-c.elevation_m.round() as i32) % 1_000) < 50;
                    let base_a = if major { 0.38_f32 } else { 0.16_f32 };
                    let a = (base_a * (0.5 + depth_norm * 0.5) * 255.0) as u8;
                    let r = (25.0 * (1.0 - depth_norm * 0.8)) as u8;
                    let g = (70.0 * (1.0 - depth_norm * 0.6)) as u8;
                    let b = (175 + (40.0 * depth_norm) as u8).min(255);
                    (egui::Color32::from_rgba_premultiplied(r, g, b, a), major)
                },
            ) {
                line_batches.push(batch);
            }
        }

        if model.show_coastlines && !atlas_earth {
            if let Some(coasts) = contour_asset::load_global_coastlines(
                selected_root,
                model.globe_view.zoom,
                painter.ctx().clone(),
            ) && let Some(batch) = cached_contour_batch(
                0xC0A5_0002,
                &coasts,
                2,
                radius_m,
                false,
                stroke_px,
                stroke_px,
                0.94,
                0.0,
                0.00015,
                |_| (egui::Color32::from_rgba_premultiplied(210, 220, 255, 110), true),
            ) {
                line_batches.push(batch);
            }
        }

        if model.show_contours && !atlas_earth {
            let major_col = theme::hot_color();
            let minor_col = theme::topo_color();
            let pal_key = (u32::from_le_bytes(major_col.to_array()) as u64)
                | ((u32::from_le_bytes(minor_col.to_array()) as u64) << 32);

            // Global topo overview when zoomed wide
            if half_deg > 4.0
                && let Some(topo) = contour_asset::load_global_topo(
                    selected_root,
                    model.globe_view.zoom,
                    painter.ctx().clone(),
                )
                && let Some(batch) = cached_contour_batch(
                    0x7090_0003,
                    &topo,
                    pal_key,
                    radius_m,
                    false,
                    stroke_px * 0.85,
                    stroke_px * 1.15,
                    0.92,
                    0.0,
                    0.00012,
                    |c| {
                        let major = (c.elevation_m.round() as i32).rem_euclid(500) == 0;
                        (if major { major_col } else { minor_col }, major)
                    },
                )
            {
                line_batches.push(batch);
            }

            // Detailed SRTM / 3DEP contour tiles when zoomed below ~6° half-extent
            if half_deg <= 6.0 {
                let render_zoom = local_terrain_scene::local_render_zoom(model.globe_view.local_zoom);
                let center = proj.target_geo.to_f32();
                let load = contour_asset::load_srtm_lod_region(
                    selected_root,
                    None,
                    center,
                    render_zoom,
                    render_zoom,
                    3,
                    painter.ctx().clone(),
                );
                let all_tiles = load
                    .tiles
                    .iter()
                    .chain(load.detail.iter().flat_map(|d| d.tiles.iter()));
                for tile in all_tiles {
                    let tag = 0x5274_0000_0000_u64
                        ^ ((tile.id.zoom_bucket as u64) << 40)
                        ^ (((tile.id.lat_bucket as u32) as u64) << 20)
                        ^ ((tile.id.lon_bucket as u32) as u64);
                    if let Some(batch) = cached_contour_batch(
                        tag,
                        &tile.contours,
                        pal_key,
                        radius_m,
                        true,
                        stroke_px * 0.75,
                        stroke_px * 1.25,
                        0.92,
                        0.0,
                        0.0002,
                        |c| {
                            let major = (c.elevation_m.round() as i32).rem_euclid(50) == 0;
                            (if major { major_col } else { minor_col }, major)
                        },
                    ) {
                        line_batches.push(batch);
                    }
                }
            }
        }

        // OSM Roads (GPU RTC lines when zoomed in below 5°)
        super::road_layer::collect_unified_batches(
            painter.ctx(),
            &proj,
            selected_root,
            model.show_major_roads,
            model.show_minor_roads,
            &mut line_batches,
        );
    } else if model.show_contours {
        // Moon / Mars contours (continuous zoom from global into SLDEM / CTX)
        let major_col = theme::hot_color();
        let minor_col = theme::topo_color();
        let pal_key = (u32::from_le_bytes(major_col.to_array()) as u64)
            | ((u32::from_le_bytes(minor_col.to_array()) as u64) << 32);
        let center = proj.target_geo.to_f32();
        let render_zoom = local_terrain_scene::local_render_zoom(model.globe_view.local_zoom);
        let load = match model.active_body {
            ActiveBody::Moon => contour_asset::load_lunar_region_for_view(
                selected_root,
                center,
                center,
                render_zoom,
                3,
                3,
                painter.ctx().clone(),
            ),
            ActiveBody::Mars => contour_asset::load_mars_region_for_view(
                selected_root,
                center,
                center,
                render_zoom,
                3,
                3,
                painter.ctx().clone(),
            ),
            ActiveBody::Earth => unreachable!(),
        };
        for tile in &load.tiles {
            let tag = 0x91A4_0000_0000_u64
                ^ ((tile.id.zoom_bucket as u64) << 40)
                ^ (((tile.id.lat_bucket as u32) as u64) << 20)
                ^ ((tile.id.lon_bucket as u32) as u64);
            if let Some(batch) = cached_contour_batch(
                tag,
                &tile.contours,
                pal_key,
                radius_m,
                half_deg <= 12.0,
                stroke_px * 0.85,
                stroke_px * 1.25,
                0.92,
                0.0,
                0.0002,
                |c| {
                    let major = (c.elevation_m.round() as i32).rem_euclid(1_000) == 0;
                    (if major { major_col } else { minor_col }, major)
                },
            ) {
                line_batches.push(batch);
            }
        }
    }

    if !line_batches.is_empty() {
        painter.add(
            UnifiedLineCallback {
                proj,
                batches: line_batches,
            }
            .into_paint_callback(),
        );
    }

    // 5. Fused Highest-Density LiDAR Point Cloud Layer (M4 hook — draws into shared Depth32Float!)
    super::point_cloud_layer::draw_unified(painter, &proj, model);

    // 6. Composite the depth-tested 3D scene (terrain + RTC vectors + LiDAR points) onto egui
    painter.add(CompositeSceneCallback { rect }.into_paint_callback());

    // 7. Georeferenced Raster Overlays via unified curved-Earth mesh
    if model.active_body == ActiveBody::Earth {
        if model.show_bathymetry
            && !atlas_earth
            && let Some(tex_id) = gebco_depth_fill::ensure_texture(painter.ctx(), selected_root)
        {
            let mesh = super::raster_mesh::unified(&proj, tex_id, [-180.0, -90.0, 180.0, 90.0]);
            if !mesh.vertices.is_empty() {
                painter.add(egui::Shape::mesh(Arc::new(mesh)));
            }
        }
        if model.show_swot_gravity {
            super::swot_layer::draw_unified(painter, &proj, selected_root);
        }
        if model.gravity.enabled {
            super::gravity_layer::draw_unified(painter, &proj, selected_root, &model.gravity.map().id);
        }
        let vis_f32 = proj
            .visible_bounds()
            .map(|b| [b[0] as f32, b[1] as f32, b[2] as f32, b[3] as f32]);
        model.global_weather.draw(
            painter.ctx(),
            rect.width().max(rect.height()) * ppp,
            || vis_f32,
            |image| image.draw_unified(painter, &proj),
        );
        if model.lidar.enabled {
            let lidar_view = if half_deg <= 4.0 {
                crate::lidar_tiles::View::Fused
            } else {
                crate::lidar_tiles::View::Coverage
            };
            model.lidar.tiles.draw(vis_f32, lidar_view, |image| {
                image.draw_unified(painter, &proj)
            });
        }
        for image in &model.satellite_layers {
            image.draw_unified(painter, &proj);
        }
    }

    // 8. Admin boundaries & close-scale OSM layers (projected cleanly via MapProjection!)
    let center_f32 = proj.target_geo.to_f32();
    let local_layout = local_terrain_scene::layout(rect);
    let render_zoom = local_terrain_scene::local_render_zoom(model.globe_view.local_zoom);

    local_terrain_scene::projection::with_unified_projection(proj, || {
        super::admin_render::draw(painter, &local_layout, model, center_f32);

        if model.active_body == ActiveBody::Earth && half_deg <= 5.0 {
            if model.viewshed.enabled {
                super::viewshed::draw_local(
                    painter,
                    selected_root,
                    &model.viewshed,
                    center_f32,
                    |geo, elev| {
                        proj.project_geo(geo.lat as f64, geo.lon as f64, elev as f64)
                            .filter(|p| p.front_facing)
                            .map(|p| p.pos)
                    },
                );
            }
            super::water_layer::draw_water(
                painter,
                &local_layout,
                &model.globe_view,
                selected_root,
                center_f32,
                render_zoom,
                model.show_water,
            );
            super::waterway_layer::draw_waterways(
                painter,
                &local_layout,
                &model.globe_view,
                selected_root,
                center_f32,
                render_zoom,
                model.show_water,
            );
            super::tree_layer::draw_trees(
                painter,
                &local_layout,
                &model.globe_view,
                selected_root,
                center_f32,
                render_zoom,
                model.show_trees && !atlas_earth,
            );
            super::building_layer::draw_buildings(
                painter,
                &local_layout,
                &model.globe_view,
                selected_root,
                center_f32,
                render_zoom,
                model.show_buildings,
            );
            super::power_layer::draw_power(
                painter,
                &local_layout,
                &model.globe_view,
                selected_root,
                center_f32,
                render_zoom,
                model.show_power,
            );
            super::infra_layer::draw_rail(
                painter,
                &local_layout,
                &model.globe_view,
                selected_root,
                center_f32,
                render_zoom,
                model.show_rail,
            );
            super::infra_layer::draw_pipelines(
                painter,
                &local_layout,
                &model.globe_view,
                selected_root,
                center_f32,
                render_zoom,
                model.show_pipeline && model.pipeline_osm,
                model.pipeline_capacity,
            );
            super::infra_layer::draw_aeroways(
                painter,
                &local_layout,
                &model.globe_view,
                selected_root,
                center_f32,
                render_zoom,
                model.show_aeroway,
            );
            super::infra_layer::draw_military(
                painter,
                &local_layout,
                &model.globe_view,
                selected_root,
                center_f32,
                render_zoom,
                model.show_military,
            );
            super::infra_layer::draw_comm(
                painter,
                &local_layout,
                &model.globe_view,
                selected_root,
                center_f32,
                render_zoom,
                model.show_comm,
            );
            super::infra_layer::draw_industrial(
                painter,
                &local_layout,
                &model.globe_view,
                selected_root,
                center_f32,
                render_zoom,
                model.show_industrial,
            );
            super::infra_layer::draw_port(
                painter,
                &local_layout,
                &model.globe_view,
                selected_root,
                center_f32,
                render_zoom,
                model.show_port,
            );
            super::infra_layer::draw_government(
                painter,
                &local_layout,
                &model.globe_view,
                selected_root,
                center_f32,
                render_zoom,
                model.show_government,
            );
            super::infra_layer::draw_surveillance(
                painter,
                &local_layout,
                &model.globe_view,
                selected_root,
                center_f32,
                render_zoom,
                model.show_surveillance,
            );
        }
    });

    // 9. Pipelines, Platforms, Submarine Cables, GhostMaps & GeoJSON overlays
    if model.active_body == ActiveBody::Earth
        && (model.show_pipeline || model.show_platforms)
        && let Some(root) = selected_root
    {
        let vis_bounds = proj
            .visible_bounds()
            .map(|b| [b[1] as f32, b[3] as f32, b[0] as f32, b[2] as f32]);
        let key = [
            proj.target_geo.lat as f32,
            proj.target_geo.lon as f32,
            proj.range_m as f32,
            proj.heading,
            proj.tilt,
            proj.projection.is_ortho() as u8 as f32,
            rect.width(),
            rect.height(),
        ]
        .map(f32::to_bits)
        .to_vec();
        if model.show_pipeline {
            super::pipeline_layer::draw(
                painter,
                root,
                vis_bounds,
                key.clone(),
                model.pipeline_filters,
                model.pipeline_capacity,
                move |point| {
                    proj.project_surface(point)
                        .filter(|p| p.front_facing)
                        .map(|p| p.pos)
                },
            );
        }
        if model.show_platforms {
            super::platform_layer::draw(
                painter,
                root,
                half_deg <= 5.0,
                key,
                model.platform_filters,
                |point| {
                    proj.project_surface(point)
                        .filter(|p| p.front_facing)
                        .map(|p| p.pos)
                },
            );
        }
    }

    let mut landing_point_markers = Vec::new();
    if model.show_submarine_cables
        && model.active_body == ActiveBody::Earth
        && !model.submarine_cable_layers.is_empty()
    {
        draw_geojson_unified(
            painter,
            &proj,
            &model.submarine_cable_layers,
            crate::submarine_cables::catalog(),
        );
        if crate::submarine_cables::landing_points_visible(model)
            && let Some(catalog) = crate::submarine_cables::catalog()
        {
            landing_point_markers = catalog
                .landings
                .iter()
                .enumerate()
                .filter_map(|(index, landing)| {
                    proj.project_surface(landing.location)
                        .filter(|p| p.front_facing)
                        .map(|p| (index, p.pos))
                })
                .collect();
            super::map_detail_panels::ring_selected_landing(painter, &landing_point_markers, model);
        }
    }
    if model.active_body == ActiveBody::Earth {
        for layers in model.ghostmaps.layers() {
            draw_geojson_unified(painter, &proj, layers, None);
        }
    }
    if !model.geojson_layers.is_empty() {
        draw_geojson_unified(painter, &proj, &model.geojson_layers, None);
    }

    // 10. Graticule, Stellar & Seismic / Live Station overlays
    if model.show_graticule {
        super::graticule::draw_unified(painter, &proj);
    }
    globe_scene::with_unified_projection(proj, || {
        if model.show_stellar_correspondence && half_deg > 10.0 {
            globe_scene::draw_stellar_unified(painter, rect, model);
        }
        globe_scene::draw_seismic_and_stations_unified(painter, rect, model);
    });

    // 11. Live Event Spires, Replay Flares, Webcams, Ships, Flights, Transit, Buildout, Fires, ALPRs
    // Scale radial spire height smoothly with viewport ground span so beams read cleanly at every scale!
    let spire_scale_m = (proj.half_h_m * 0.14).clamp(15.0, 950_000.0) * (model.marker_scale() as f64);
    let selected_event_id = model.selected_event_id.as_deref();
    let selected_camera_id = model.selected_camera_id.as_deref();

    if model.active_body == ActiveBody::Earth
        && let Some(state) = model.active_replay()
    {
        let wall_elapsed = state.wall_elapsed();
        for flare in &state.active_flares {
            let Some(base) = proj.project_surface(flare.event.location).filter(|p| p.front_facing) else {
                continue;
            };
            let tip = proj
                .project_elevated_radial(
                    flare.event.location.lat as f64,
                    flare.event.location.lon as f64,
                    0.0,
                    spire_scale_m,
                )
                .map(|p| p.pos)
                .unwrap_or(base.pos);
            globe_scene::draw_replay_flare_pub(
                painter,
                ProjectedPoint {
                    pos: base.pos,
                    depth: base.depth_ndc,
                    front_facing: true,
                },
                tip,
                flare,
                wall_elapsed,
                model.marker_scale(),
            );
        }
    }

    let event_markers: Vec<MapMarker> = if !model.show_event_markers
        || model.replay_mode
        || model.follow_recaps.active.is_some()
        || model.active_body != ActiveBody::Earth
    {
        Vec::new()
    } else {
        model
            .map_events()
            .filter_map(|event| {
                let base = proj.project_surface(event.location).filter(|p| p.front_facing)?;
                let tip = proj
                    .project_elevated_radial(
                        event.location.lat as f64,
                        event.location.lon as f64,
                        0.0,
                        spire_scale_m,
                    )
                    .map(|p| p.pos)
                    .unwrap_or(base.pos);
                globe_scene::draw_event_marker_pub(
                    painter,
                    ProjectedPoint {
                        pos: base.pos,
                        depth: base.depth_ndc,
                        front_facing: true,
                    },
                    tip,
                    event,
                    selected_event_id == Some(event.id.as_str()),
                    time,
                    model.marker_scale(),
                );
                Some(MapMarker::new(&event.id, base.pos, tip, model.marker_scale()))
            })
            .collect()
    };

    let camera_markers: Vec<MapMarker> = if model.active_body != ActiveBody::Earth
        || !model.show_camera_markers
    {
        Vec::new()
    } else {
        model
            .cameras
            .iter()
            .filter_map(|camera| {
                let base = proj.project_surface(camera.location).filter(|p| p.front_facing)?;
                let tip = proj
                    .project_elevated_radial(
                        camera.location.lat as f64,
                        camera.location.lon as f64,
                        0.0,
                        spire_scale_m * 0.7,
                    )
                    .map(|p| p.pos)
                    .unwrap_or(base.pos);
                draw_camera_spire(
                    painter,
                    base.pos,
                    tip,
                    selected_camera_id == Some(camera.id.as_str()),
                    model.marker_scale(),
                );
                Some(MapMarker::new(&camera.id, base.pos, tip, model.marker_scale()))
            })
            .collect()
    };

    let buildout_markers =
        crate::buildout::markers::draw(painter, &model.buildout, model.active_body, |point| {
            proj.project_surface(point)
                .filter(|p| p.front_facing)
                .map(|p| p.pos)
        });

    if model.show_active_fires
        && model.active_body == ActiveBody::Earth
        && !model.active_fires.is_empty()
    {
        let projected: Vec<_> = model
            .active_fires
            .iter()
            .filter_map(|d| {
                proj.project_surface(d.location)
                    .filter(|p| p.front_facing && rect.expand(16.0).contains(p.pos))
                    .map(|p| super::fire_layer::ProjectedFire::new(p.pos, d))
            })
            .collect();
        if !projected.is_empty() {
            super::fire_layer::draw_mesh(painter, super::fire_layer::build_mesh(&projected));
        }
    }

    if model.active_body == ActiveBody::Earth && (model.show_deflock_alprs || model.flock.show) {
        let step_deg = (half_deg * 0.05).clamp(0.0005, 0.05);
        let projected: Vec<_> = model
            .public_camera_positions()
            .filter_map(|(loc, dir)| {
                let p = proj
                    .project_surface(loc)
                    .filter(|p| p.front_facing && rect.expand(16.0).contains(p.pos))?;
                let screen_dir = deflock_layer::bearing_target(loc, dir, step_deg)
                    .and_then(|t| proj.project_surface(t).filter(|q| q.front_facing))
                    .and_then(|q| {
                        let d = q.pos - p.pos;
                        (d.length_sq() > f32::EPSILON).then(|| d.y.atan2(d.x))
                    });
                Some(deflock_layer::ProjectedAlprMarker::new(p.pos, screen_dir))
            })
            .collect();
        if !projected.is_empty() {
            deflock_layer::draw_mesh(painter, deflock_layer::build_mesh(&projected));
        }
    }

    let transit_markers = crate::transit::draw(painter, rect, model, |geo| {
        proj.project_surface(geo)
            .filter(|p| p.front_facing)
            .map(|p| p.pos)
    });

    // Ships & Flights now stay visible at ALL zoom levels!
    let projected_ships: Vec<(usize, ProjectedPoint)> = if model.show_ships
        && model.active_body == ActiveBody::Earth
    {
        model
            .tracks
            .iter()
            .enumerate()
            .filter_map(|(i, track)| {
                proj.project_surface(track.location)
                    .filter(|p| p.front_facing)
                    .map(|p| {
                        (
                            i,
                            ProjectedPoint {
                                pos: p.pos,
                                depth: p.depth_ndc,
                                front_facing: true,
                            },
                        )
                    })
            })
            .collect()
    } else {
        Vec::new()
    };
    if !projected_ships.is_empty() {
        globe_scene::draw_ships_pub(painter, &model.tracks, &projected_ships, model.selected_track_mmsi);
    }
    let ship_markers: Vec<(u64, egui::Pos2)> = projected_ships
        .iter()
        .map(|(i, p)| (model.tracks[*i].mmsi, p.pos))
        .collect();

    let projected_flights: Vec<(usize, ProjectedPoint)> = if model.show_flights
        && model.active_body == ActiveBody::Earth
    {
        model
            .flights
            .iter()
            .enumerate()
            .filter_map(|(i, flight)| {
                proj.project_surface(flight.location)
                    .filter(|p| p.front_facing)
                    .map(|p| {
                        (
                            i,
                            ProjectedPoint {
                                pos: p.pos,
                                depth: p.depth_ndc,
                                front_facing: true,
                            },
                        )
                    })
            })
            .collect()
    } else {
        Vec::new()
    };
    if !projected_flights.is_empty() {
        globe_scene::draw_flights_pub(
            painter,
            &model.flights,
            &projected_flights,
            model.selected_flight_icao24.as_deref(),
        );
    }
    let flight_markers: Vec<(String, egui::Pos2)> = projected_flights
        .iter()
        .map(|(i, p)| (model.flights[*i].icao24.clone(), p.pos))
        .collect();

    let arcgis_feature_markers = if !model.arcgis_features.is_empty() {
        draw_arcgis_unified(
            painter,
            &proj,
            &model.arcgis_features,
            model.selected_arcgis_feature.as_ref(),
        )
    } else {
        Vec::new()
    };

    // Agent annotations
    for ann in &model.agent_annotations {
        if let Some(pt) = proj.project_surface(ann.location).filter(|p| p.front_facing) {
            let col = theme::topo_color();
            painter.circle_stroke(pt.pos, 8.0, egui::Stroke::new(1.5, col));
            painter.circle_filled(pt.pos, 3.0, egui::Color32::WHITE);
            painter.text(
                pt.pos + egui::vec2(10.0, -4.0),
                egui::Align2::LEFT_CENTER,
                &ann.label,
                egui::FontId::proportional(11.0),
                col,
            );
        }
    }

    // HUD frame, reticle & continuous scale legend
    if !model.obs_view {
        painter.rect_stroke(
            rect.shrink(6.0),
            12.0,
            egui::Stroke::new(0.7, theme::topo_color().gamma_multiply(0.45)),
            egui::StrokeKind::Outside,
        );
        if model.show_reticle {
            draw_hud_corners(painter, rect);
        }
        draw_unified_legend(painter, rect, &proj, model);
    }

    let beam_elev = if model.active_body == ActiveBody::Earth {
        super::srtm_stream::sample_elevation_m(selected_root, center_f32)
    } else {
        None
    };

    GlobeScene {
        event_markers,
        camera_markers,
        ship_markers,
        flight_markers,
        transit_markers,
        arcgis_feature_markers,
        landing_point_markers,
        buildout_markers,
        beam_elevation_m: beam_elev,
    }
}

fn draw_geojson_unified(
    painter: &egui::Painter,
    proj: &MapProjection,
    layers: &[GeoJsonLayer],
    cables: Option<&crate::submarine_cables::CableCatalog>,
) {
    let pointer = super::infrastructure_hover::pointer();
    for layer in layers {
        if !layer.visible {
            continue;
        }
        let col = egui::Color32::from_rgba_unmultiplied(
            layer.color[0],
            layer.color[1],
            layer.color[2],
            layer.color[3],
        );
        let stroke = egui::Stroke::new(1.2, col);
        for (feat_idx, feat) in layer.features.iter().enumerate() {
            match &feat.geometry {
                GeoJsonGeometry::Point(pt) => {
                    if let Some(p) = proj.project_surface(*pt).filter(|p| p.front_facing) {
                        painter.circle_filled(p.pos, 3.0, col);
                    }
                }
                GeoJsonGeometry::LineString(pts) => {
                    draw_geo_polyline_unified(painter, proj, pts, stroke, pointer, cables, feat_idx);
                }
                GeoJsonGeometry::MultiLineString(lines) => {
                    for pts in lines {
                        draw_geo_polyline_unified(painter, proj, pts, stroke, pointer, cables, feat_idx);
                    }
                }
                GeoJsonGeometry::Polygon(rings) => {
                    for ring in rings {
                        draw_geo_polyline_unified(painter, proj, ring, stroke, pointer, None, 0);
                    }
                }
                GeoJsonGeometry::MultiPolygon(polys) => {
                    for rings in polys {
                        for ring in rings {
                            draw_geo_polyline_unified(painter, proj, ring, stroke, pointer, None, 0);
                        }
                    }
                }
            }
        }
    }
}

fn draw_geo_polyline_unified(
    painter: &egui::Painter,
    proj: &MapProjection,
    pts: &[GeoPoint],
    stroke: egui::Stroke,
    pointer: Option<egui::Pos2>,
    cables: Option<&crate::submarine_cables::CableCatalog>,
    feat_idx: usize,
) {
    let mut seg = Vec::new();
    let mut best_dist = f32::INFINITY;
    for pt in pts {
        if let Some(p) = proj.project_surface(*pt).filter(|p| p.front_facing) {
            if let (Some(ptr), Some(&prev)) = (pointer, seg.last()) {
                best_dist = best_dist.min(super::infrastructure_hover::line_distance(ptr, prev, p.pos));
            }
            seg.push(p.pos);
        } else {
            if seg.len() >= 2 {
                painter.add(egui::Shape::line(std::mem::take(&mut seg), stroke));
            } else {
                seg.clear();
            }
        }
    }
    if seg.len() >= 2 {
        painter.add(egui::Shape::line(seg, stroke));
    }
    if best_dist <= 8.0
        && let Some(catalog) = cables
        && let Some(info) = catalog.cables.get(feat_idx)
    {
        super::infrastructure_hover::cable(info, best_dist);
    }
}

fn draw_arcgis_unified(
    painter: &egui::Painter,
    proj: &MapProjection,
    features: &[ArcGisFeature],
    selected: Option<&(String, i64)>,
) -> Vec<(String, i64, egui::Pos2)> {
    let mut out = Vec::new();
    for feat in features {
        let Some(p) = proj.project_surface(feat.location).filter(|p| p.front_facing) else {
            continue;
        };
        let col = arcgis_source::feature_color(feat);
        let has_cas = feat.has_casualties();
        let is_selected = selected
            .map(|(u, id)| u == &feat.source_url && *id == feat.object_id)
            .unwrap_or(false);
        if is_selected {
            painter.circle_stroke(
                p.pos,
                if has_cas { 13.0 } else { 11.0 },
                egui::Stroke::new(1.5, egui::Color32::WHITE),
            );
        }
        painter.circle_stroke(
            p.pos,
            if has_cas { 9.0 } else { 6.5 },
            egui::Stroke::new(2.5, col.gamma_multiply(0.14)),
        );
        painter.circle_filled(p.pos, if has_cas { 3.5 } else { 2.5 }, col);
        out.push((feat.source_url.clone(), feat.object_id, p.pos));
    }
    out
}

fn draw_hud_corners(painter: &egui::Painter, rect: egui::Rect) {
    for &(x, y, dx, dy) in &[
        (rect.left() + 18.0, rect.top() + 18.0, 28.0, 16.0),
        (rect.right() - 18.0, rect.top() + 18.0, -28.0, 16.0),
        (rect.left() + 18.0, rect.bottom() - 18.0, 28.0, -16.0),
        (rect.right() - 18.0, rect.bottom() - 18.0, -28.0, -16.0),
    ] {
        painter.line_segment(
            [egui::pos2(x, y), egui::pos2(x + dx, y)],
            egui::Stroke::new(1.0, theme::topo_color()),
        );
        painter.line_segment(
            [egui::pos2(x, y), egui::pos2(x, y + dy)],
            egui::Stroke::new(1.0, theme::topo_color()),
        );
    }
}

fn draw_unified_legend(
    painter: &egui::Painter,
    rect: egui::Rect,
    proj: &MapProjection,
    model: &AppModel,
) {
    let body_name = match model.active_body {
        ActiveBody::Earth => "EARTH",
        ActiveBody::Moon => "MOON",
        ActiveBody::Mars => "MARS",
    };
    let tilt_deg = proj.tilt.to_degrees().round() as i32;
    let hdg_deg = (proj.heading.to_degrees().rem_euclid(360.0)).round() as i32;
    painter.text(
        egui::pos2(rect.left() + 24.0, rect.bottom() - 78.0),
        egui::Align2::LEFT_TOP,
        format!(
            "{body_name} · {}\nSPAN {} · {:.1} m/px\nHDG {hdg_deg:03}° · TILT {tilt_deg}°",
            proj.projection.label(),
            proj.scale_label(),
            proj.meters_per_pixel(),
        ),
        egui::FontId::monospace(11.5),
        theme::text_muted(),
    );
}
