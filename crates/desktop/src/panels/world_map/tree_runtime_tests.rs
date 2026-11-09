use super::*;
#[path = "admin_test_screen.rs"]
mod screen;

#[test]
#[ignore = "requires GPU, GDAL, existing Alpine terrain and cached forest cells"]
fn atlas_with_native_forests_renders_bounded_tree_geometry() {
    use std::time::{Duration, Instant};
    let root =
        std::path::PathBuf::from(std::env::var("ONEKEE_ATLAS_ROOT").expect("existing source root"));
    let mut gpu = screen::Screen::new([1280, 832]);
    let ctx = egui::Context::default();
    crate::theme::set_theme(&ctx, crate::theme::MapTheme::Atlas);
    let mut model = crate::model::AppModel::new();
    model.map_theme = crate::theme::MapTheme::Atlas;
    model.atlas_style = crate::atlas_style::AtlasStyle::default();
    model.selected_root = Some(root.clone());
    let focus = GeoPoint {
        lat: 46.5,
        lon: 7.5,
    };
    model.globe_view = GlobeViewState::from_focus(focus);
    model.globe_view.local_mode = true;
    model.globe_view.local_zoom = 8.;
    model.globe_view.local_layer_spread = 8.;
    model.show_contours = false;
    model.show_bathymetry = false;
    model.show_coastlines = false;
    model.show_trees = false;
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280., 832.));
    let layout = LocalLayout {
        center: rect.center(),
        focus_center: rect.center() + egui::vec2(rect.width() * 0.02, 12.),
        width: rect.width() * 0.82,
        height: rect.height() * 0.74,
        horizontal_scale: rect.width() * 0.31,
    };
    let half = visual_half_extent_for_zoom(model.globe_view.local_zoom);
    let maximum = gpu.max_texture_side().min(8192);
    let start = Instant::now();
    let mut baseline = None;
    let mut settled = false;
    let mut forest_seen = false;
    let mut terrain_id = None;
    loop {
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(rect),
                max_texture_side: Some(maximum),
                ..Default::default()
            },
            |ctx| {
                let painter = ctx
                    .layer_painter(egui::LayerId::background())
                    .with_clip_rect(rect);
                painter.rect_filled(rect, 0., crate::theme::canvas_background());
                super::super::super::atlas_relief::draw_local(
                    &painter, &layout, &model, focus, half,
                );
                draw_trees(
                    &painter,
                    &layout,
                    &model.globe_view,
                    Some(&root),
                    focus,
                    8.,
                    false, // Atlas paints forest coverage on the solid terrain.
                );
            },
        );
        let pixels = gpu.render(&ctx, output, true).unwrap();
        let ready = ctx
            .data(|d| d.get_temp::<u64>(egui::Id::new(("atlas-surface-wanted", 1usize))))
            .is_some_and(|id| super::super::super::atlas_surface::uploaded(&ctx, 1, id));
        if ready {
            if !settled {
                settled = true;
                continue;
            }
            if baseline.is_none() {
                if let Ok(path) = std::env::var("ONEKEE_ATLAS_TREES_BASE_PREVIEW") {
                    image::save_buffer(path, &pixels, 1280, 832, image::ColorType::Rgba8).unwrap();
                }
                baseline = Some(pixels);
                model.show_trees = true;
                continue;
            }
            let active =
                ctx.data(|d| d.get_temp::<u64>(egui::Id::new(("atlas-surface-active", 1usize))));
            if terrain_id.is_none() {
                terrain_id = active;
            }
            assert_eq!(terrain_id, active, "Trees never reloads terrain geometry");
            let mask_ready = ctx
                .data(|d| d.get_temp::<u64>(egui::Id::new("atlas-forest-wanted")))
                .is_some_and(|id| super::super::super::atlas_forest::uploaded(&ctx, id));
            if model.show_trees && mask_ready {
                let changed = baseline
                    .as_ref()
                    .unwrap()
                    .chunks_exact(4)
                    .zip(pixels.chunks_exact(4))
                    .filter(|(a, b)| a != b)
                    .count();
                assert!(
                    changed > 1000,
                    "real forest coverage changes the Atlas terrain material"
                );
                if let Ok(path) = std::env::var("ONEKEE_ATLAS_TREES_PREVIEW") {
                    image::save_buffer(path, &pixels, 1280, 832, image::ColorType::Rgba8).unwrap();
                }
                println!("Rendered actual forest coverage; {changed} changed pixels");
                model.show_trees = false;
                forest_seen = true;
                continue;
            }
            if forest_seen && !model.show_trees {
                assert_eq!(
                    Some(&pixels),
                    baseline.as_ref(),
                    "disabling Trees restores the exact original terrain"
                );
                break;
            }
        }
        assert!(
            start.elapsed() < Duration::from_secs(90),
            "native trees never published"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    tree_cache().lock().unwrap().cache = None;
    super::super::super::atlas_relief::clear();
    crate::theme::set_theme(&ctx, crate::theme::MapTheme::Topo);
}
