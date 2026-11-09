//! Exercise the production draw/selection/load path, not a fixed preview crop.
use super::*;

#[path = "admin_test_screen.rs"]
mod screen;

#[test]
fn unavailable_detail_is_reported_without_blocking_the_canvas() {
    let scratch = tempfile::tempdir().unwrap();
    std::fs::create_dir(scratch.path().join("Data")).unwrap();
    let ctx = egui::Context::default();
    let region = super::super::atlas_viewport::Region([0, 0, 900, 900]);
    clear();
    let start = std::time::Instant::now();
    assert!(detail_texture(&ctx, Some(scratch.path()), region, false).is_none());
    loop {
        detail_texture(&ctx, Some(scratch.path()), region, false);
        let notice =
            ctx.data(|d| d.get_temp::<DetailNotice>(egui::Id::new(("atlas-notice", false))));
        if let Some(notice) = notice {
            let message = notice.message.expect("source failure remains visible");
            assert!(message.contains("GEBCO") || message.contains("terrain data"));
            break;
        }
        assert!(start.elapsed() < std::time::Duration::from_secs(5));
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    clear();
}

#[test]
#[ignore = "requires GPU, GDAL and original terrain; run with Finder-style PATH"]
fn native_globe_runtime_refines_after_gui_launch() {
    use std::time::{Duration, Instant};
    let root = PathBuf::from(std::env::var("ONEKEE_ATLAS_ROOT").expect("existing terrain root"));
    let mut gpu = screen::Screen::new([1280, 832]);
    gpu.install_globe();
    let maximum = gpu.max_texture_side().min(8192);
    let ctx = egui::Context::default();
    crate::theme::set_theme(&ctx, crate::theme::MapTheme::Atlas);
    let mut model = crate::model::AppModel::new();
    model.map_theme = crate::theme::MapTheme::Atlas;
    model.atlas_style = crate::atlas_style::AtlasStyle::default();
    model.selected_root = Some(root);
    model.globe_view = GlobeViewState::from_focus(crate::model::GeoPoint {
        lat: 46.0,
        lon: 7.5,
    });
    model.globe_view.zoom = 28.67;
    model.show_contours = false;
    model.show_coastlines = false;
    model.show_bathymetry = false;
    model.show_event_markers = false;
    model.show_camera_markers = false;
    model.show_beam = false;
    model.obs_view = true;
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280., 832.));
    clear();
    let start = Instant::now();
    let mut coarse = None;
    let mut refined = None;
    let mut settled = false;
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
                super::super::globe_scene::paint(&painter, rect, &model, 0.0);
            },
        );
        let overview = IMAGE.get().unwrap().lock().unwrap().current().is_some();
        let tiles = super::super::atlas_globe_tiles::frame(
            &ctx,
            model.selected_root.as_deref(),
            rect,
            &super::super::globe_scene::globe_layout(rect, &model.globe_view),
            &model.globe_view,
        );
        let detail = if tiles.missing == 0 {
            tiles
                .rasters
                .last()
                .map(|r| LoadedDetail { raster: r.clone() })
        } else {
            None
        };
        let capture = detail.is_some() || (overview && coarse.is_none());
        let pixels = gpu.render(&ctx, output, capture);
        if let Some(detail) =
            detail.filter(|detail| super::super::atlas_surface::uploaded(&ctx, 0, detail.raster.id))
        {
            if !settled {
                settled = true;
                continue;
            }
            if let Some((id, original)) = refined.as_ref() {
                assert_eq!(
                    *id, detail.raster.id,
                    "moving light retains the native source"
                );
                let pixels = pixels.unwrap();
                assert_ne!(
                    &pixels, original,
                    "production Globe responds to light direction"
                );
                if let Ok(path) = std::env::var("ONEKEE_ATLAS_MOVED_LIGHT_PREVIEW") {
                    image::save_buffer(path, &pixels, 1280, 832, image::ColorType::Rgba8).unwrap();
                }
                break;
            }
            let [west, _, east, _] = detail.raster.bounds;
            assert!(
                detail.raster.size[0] as f32 / (east - west) > 150.,
                "actual selected detail has native-scale resolution"
            );
            let pixels = pixels.unwrap();
            assert_ne!(
                coarse.as_ref(),
                Some(&pixels),
                "production refinement replaces the coarse image"
            );
            if let Ok(path) = std::env::var("ONEKEE_ATLAS_RUNTIME_PREVIEW") {
                image::save_buffer(path, &pixels, 1280, 832, image::ColorType::Rgba8).unwrap();
            }
            assert!(
                !model.show_contours,
                "rendering preserves the operator's contour choice"
            );
            refined = Some((detail.raster.id, pixels));
            model.atlas_style.azimuth_deg = 135.0;
            continue;
        }
        if overview && coarse.is_none() {
            coarse = pixels;
        }
        assert!(
            tiles.error.is_none(),
            "runtime detail failed: {:?}",
            tiles.error
        );
        assert!(
            start.elapsed() < Duration::from_secs(90),
            "runtime detail never published"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    clear();
    crate::theme::set_theme(&ctx, crate::theme::MapTheme::Topo);
}

#[test]
#[ignore = "requires GPU, GDAL and original terrain; production Local controller"]
fn native_local_runtime_draws_solid_terrain_and_attached_contours() {
    use std::time::{Duration, Instant};
    let root = PathBuf::from(std::env::var("ONEKEE_ATLAS_ROOT").expect("terrain root"));
    let mut gpu = screen::Screen::new([1280, 832]);
    let maximum = gpu.max_texture_side().min(8192);
    let ctx = egui::Context::default();
    crate::theme::set_theme(&ctx, crate::theme::MapTheme::Atlas);
    let mut model = crate::model::AppModel::new();
    model.map_theme = crate::theme::MapTheme::Atlas;
    model.atlas_style = crate::atlas_style::AtlasStyle::default();
    model.selected_root = Some(root);
    let focus = GeoPoint {
        lat: 46.5,
        lon: 7.5,
    };
    model.globe_view = GlobeViewState::from_focus(focus);
    model.globe_view.local_mode = true;
    model.globe_view.local_pitch = 1.0;
    model.globe_view.local_layer_spread = 2.5;
    model.show_contours = false;
    model.show_bathymetry = false;
    model.show_coastlines = false;
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280., 832.));
    let layout = LocalLayout {
        center: rect.center(),
        focus_center: rect.center() + egui::vec2(rect.width() * 0.02, 12.),
        width: rect.width() * 0.82,
        height: rect.height() * 0.74,
        horizontal_scale: rect.width() * 0.31,
    };
    clear();
    let started = Instant::now();
    let mut solid = None;
    let mut settled = false;
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
                draw_local(&painter, &layout, &model, focus, 0.4);
            },
        );
        let pixels = gpu.render(&ctx, output, true).unwrap();
        let data = LOCAL_DETAIL
            .get()
            .unwrap()
            .lock()
            .unwrap()
            .current()
            .map(|(_, d)| d.clone());
        if let Some(data) = data
            && super::super::atlas_surface::uploaded(&ctx, 1, data.raster.id)
        {
            // Let the CPU-side publication notice catch up before comparing
            // the two views, so the assertion measures the contour toggle.
            if !settled {
                settled = true;
                continue;
            }
            if let Some(solid) = solid {
                assert_ne!(
                    pixels, solid,
                    "optional contours alter the already solid terrain"
                );
                if let Ok(path) = std::env::var("ONEKEE_ATLAS_LOCAL_CONTOURS_PREVIEW") {
                    image::save_buffer(path, &pixels, 1280, 832, image::ColorType::Rgba8).unwrap();
                }
                break;
            }
            if let Ok(path) = std::env::var("ONEKEE_ATLAS_LOCAL_3D_PREVIEW") {
                image::save_buffer(path, &pixels, 1280, 832, image::ColorType::Rgba8).unwrap();
            }
            solid = Some(pixels);
            model.show_contours = true;
        }
        assert!(
            started.elapsed() < Duration::from_secs(90),
            "Local 3D terrain did not publish"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(model.show_contours);
    clear();
    crate::theme::set_theme(&ctx, crate::theme::MapTheme::Topo);
}
