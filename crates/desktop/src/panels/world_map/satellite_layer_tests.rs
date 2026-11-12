//! Exercise the file-import worker and both production scenes with a local display product.
use super::*;
use crate::panels::layer_file_import;
use std::time::{Duration, Instant};

#[path = "admin_test_screen.rs"]
mod screen;

#[test]
#[ignore = "requires GPU, GDAL and a visible regional GeoTIFF in ONEKEE_TEST_PASS"]
fn imported_satellite_pixels_reach_both_earth_scenes() {
    let path = std::env::var_os("ONEKEE_TEST_PASS").expect("path to projected GeoTIFF");
    let ctx = egui::Context::default();
    let mut gpu = screen::Screen::new([1024, 640]);
    gpu.install_globe();
    let mut model = AppModel::new();
    layer_file_import::queue(path.into(), &mut model, gpu.max_texture_side());
    let start = Instant::now();
    while layer_file_import::busy() {
        layer_file_import::poll(&ctx, &mut model);
        assert!(
            start.elapsed() < Duration::from_secs(150),
            "import timed out"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(model.satellite_layers.len(), 1, "{:?}", model.activity_log);
    model
        .globe_view
        .focus_on(model.satellite_layers[0].center());
    model.globe_view.zoom = 40.0;
    model.globe_view.local_zoom = 3.0;
    model.globe_view.local_yaw = 0.0;
    model.globe_view.local_pitch = 0.35;
    model.map_theme = crate::theme::MapTheme::Topo;
    model.obs_view = true;
    model.selected_root = None;
    model.show_contours = false;
    model.show_bathymetry = false;
    model.show_event_markers = false;
    model.show_camera_markers = false;
    model.show_beam = false;
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1024.0, 640.0));
    for local in [false, true] {
        model.globe_view.local_mode = local;
        let mut baseline = None;
        for visible in [false, true] {
            model.satellite_layers[0].visible = visible;
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(rect),
                    max_texture_side: Some(gpu.max_texture_side()),
                    ..Default::default()
                },
                |ctx| {
                    let painter = ctx
                        .layer_painter(egui::LayerId::background())
                        .with_clip_rect(rect);
                    if local {
                        super::super::local_terrain_scene::paint(&painter, rect, &model, 0.0);
                    } else {
                        super::super::globe_scene::paint(&painter, rect, &model, 0.0);
                    }
                },
            );
            let id = model.satellite_layers[0].texture.id();
            assert_eq!(visible, output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Mesh(mesh) if mesh.texture_id == id && !mesh.indices.is_empty())));
            let pixels = gpu.render(&ctx, output, true).unwrap();
            if let Some(background) = baseline.as_ref() {
                assert_ne!(&pixels, background, "overlay did not reach the framebuffer");
                if let Some(dir) = std::env::var_os("ONEKEE_PASS_PREVIEWS") {
                    let file = if local { "local.png" } else { "globe.png" };
                    image::save_buffer(
                        std::path::PathBuf::from(dir).join(file),
                        &pixels,
                        1024,
                        640,
                        image::ColorType::Rgba8,
                    )
                    .unwrap();
                }
            } else {
                baseline = Some(pixels);
            }
        }
    }
}

#[test]
fn lidar_picking_inverts_the_rendered_globe_and_oblique_local_ground() {
    use super::super::{globe_scene, lidar_pick_region, local_terrain_scene};
    let rect = egui::Rect::from_min_size(egui::pos2(30., 72.), egui::vec2(1024., 640.));
    for center in [
        GeoPoint {
            lon: -75.09,
            lat: 38.33,
        },
        GeoPoint {
            lon: 179.99,
            lat: -84.,
        },
    ] {
        let mut view = GlobeViewState::from_focus(center);
        view.local_zoom = 20.;
        view.local_pitch = 0.8;
        view.local_yaw = 1.2;
        let point = GeoPoint {
            lon: (center.lon + 0.02 + 180.).rem_euclid(360.) - 180.,
            lat: center.lat + 0.012,
        };
        let layout = globe_scene::globe_layout(rect, &view);
        let globe = globe_scene::project_surface(&layout, &view, point)
            .unwrap()
            .pos;
        let local = raster_mesh::ground_projection(
            &local_terrain_scene::layout(rect),
            &view,
            center,
            local_terrain_scene::visual_half_extent_for_zoom(view.local_zoom),
        );
        let (x, y, _) = local.project_logical(point, 0.);
        for (local, pos) in [(false, globe), (true, egui::pos2(x, y))] {
            let picked = lidar_pick_region(rect, &view, local, pos).unwrap().at;
            let lon_error = (picked.lon - point.lon + 180.).rem_euclid(360.) - 180.;
            assert!(
                lon_error.abs() < 0.001 && (picked.lat - point.lat).abs() < 0.001,
                "{picked:?}, {point:?}"
            );
        }
        assert!(lidar_pick_region(rect, &view, false, rect.left_top()).is_none());
    }
}

/// Run against a catalog restricted to NOAAID:38 to check both the actual survey and the viewport
/// that each scene requests. A second run reads the saved tile set with the network disabled.
#[test]
#[ignore = "requires GPU and NOAA survey 38 in ONEKEE_TEST_LIDAR_DB"]
fn lidar_zoom_paints_measured_pixels_in_both_earth_views() {
    let db = std::path::PathBuf::from(std::env::var_os("ONEKEE_TEST_LIDAR_DB").unwrap());
    let ctx = egui::Context::default();
    let mut gpu = screen::Screen::new([1024, 640]);
    gpu.install_globe();
    let mut model = AppModel::new();
    model.lidar.set_path(db.clone());
    model.globe_view.focus_on(GeoPoint {
        lon: -75.075,
        lat: 38.33,
    });
    model.globe_view.stop_motion();
    model.globe_view.look_down();
    model.globe_view.zoom = 50.;
    model.globe_view.local_zoom = 28.;
    model.selected_root = None;
    model.obs_view = true;
    model.map_theme = crate::theme::MapTheme::Topo;
    model.show_contours = false;
    model.show_event_markers = false;
    model.show_camera_markers = false;
    model.show_bathymetry = false;
    model.show_beam = false;
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1024., 640.));
    for local in [false, true] {
        model.globe_view.local_mode = local;
        model.lidar.enabled = true;
        let frame = |model: &AppModel| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(rect),
                    ..Default::default()
                },
                |ctx| {
                    let painter = ctx
                        .layer_painter(egui::LayerId::background())
                        .with_clip_rect(rect);
                    if local {
                        super::super::local_terrain_scene::paint(&painter, rect, model, 0.);
                    } else {
                        super::super::globe_scene::paint(&painter, rect, model, 0.);
                    }
                },
            )
        };
        // Present the initial frame so font/texture uploads reach the renderer.
        gpu.render(&ctx, frame(&model), false);
        let start = Instant::now();
        loop {
            model.lidar.tiles.tick(&ctx, &db, Some("NOAAID:38"));
            if model.lidar.tiles.visible_tiles_loaded()
                && model.lidar.tiles.sources.contains("Winter ATM")
            {
                break;
            }
            assert!(
                start.elapsed() < Duration::from_secs(240),
                "local={local}: {}",
                model.lidar.tiles.status
            );
            std::thread::sleep(Duration::from_millis(30));
        }
        let output = frame(&model);
        let managers = ctx.tex_manager();
        let textures = managers.read();
        assert!(output.shapes.iter().any(|s| matches!(&s.shape, egui::Shape::Mesh(mesh)
            if !mesh.indices.is_empty() && textures.meta(mesh.texture_id).is_some_and(|meta| meta.name.contains("Winter ATM")))),
            "local={local}: measured texture absent from scene");
        drop(textures);
        let actual = gpu.render(&ctx, output, true).unwrap();
        model.lidar.tiles.tick(&ctx, &db, None);
        let background = gpu.render(&ctx, frame(&model), true).unwrap();
        assert!(
            actual != background,
            "local={local}: measurements did not add pixels to the survey outlines"
        );
        if let Some(dir) = std::env::var_os("ONEKEE_LIDAR_PREVIEWS") {
            let name = if local {
                "ocean-city-local.png"
            } else {
                "ocean-city-globe.png"
            };
            image::save_buffer(
                std::path::PathBuf::from(dir).join(name),
                &actual,
                1024,
                640,
                image::ColorType::Rgba8,
            )
            .unwrap();
        }
    }
}
