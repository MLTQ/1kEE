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
