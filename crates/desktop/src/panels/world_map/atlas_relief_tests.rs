use super::*;

#[test]
fn atlas_globe_shader_validates_without_a_graphics_device() {
    use eframe::wgpu::{ShaderSource, naga};
    let ShaderSource::Wgsl(source) = super::super::gpu_color::shader(include_str!("globe.wgsl"))
    else {
        panic!("WGSL globe shader");
    };
    let module = naga::front::wgsl::parse_str(&source).expect("globe shader parses");
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .expect("globe shader validates");
}

#[test]
fn preview_formats_decode_metres_without_unsigned_ocean_artifacts() {
    let temp = tempfile::tempdir().unwrap();
    let heights: Vec<i16> = vec![-4000, -200, 0, 200, 2000, 8000].repeat(3);
    let path = temp.path().join("signed.tif");
    let image = image::ImageBuffer::<image::Luma<u16>, _>::from_raw(
        6,
        3,
        heights.iter().map(|&v| v as u16).collect::<Vec<_>>(),
    )
    .unwrap();
    image.save(&path).unwrap();
    let grid = read_preview(&path, true).unwrap();
    assert_eq!(
        grid.metres,
        heights.iter().map(|&h| h as f32).collect::<Vec<_>>()
    );
    let png = temp.path().join("scaled.png");
    let image = image::ImageBuffer::<image::Luma<u16>, _>::from_raw(
        6,
        3,
        heights
            .iter()
            .map(|&v| (((v as f32 + 11000.0) / 20000.0) * 65535.0).round() as u16)
            .collect::<Vec<_>>(),
    )
    .unwrap();
    image.save(&png).unwrap();
    let grid = read_preview(&png, false).unwrap();
    for (&h, decoded) in heights.iter().zip(grid.metres) {
        assert!((h as f32 - decoded).abs() < 0.2);
    }
    assert!(grid.width == 6 && grid.height == 3);
}

#[path = "../settings_appearance.rs"]
mod appearance;
#[path = "admin_test_screen.rs"]
mod screen;

#[test]
fn local_dateline_quarters_keep_coverage_and_source_uvs() {
    let clip = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1024., 768.));
    let layout = LocalLayout {
        center: clip.center(),
        focus_center: clip.center(),
        width: 840.,
        height: 570.,
        horizontal_scale: 317.,
    };
    for longitude in [-179.9, 179.9] {
        let focus = GeoPoint {
            lat: 20.,
            lon: longitude,
        };
        let view = GlobeViewState::from_focus(focus);
        let mesh = local_mesh(&layout, &view, egui::TextureId::Managed(0), focus, 2., clip);
        assert!(!mesh.indices.is_empty());
        // Both ends of the same global image are present beside one another.
        assert!(mesh.vertices.iter().any(|v| v.uv.x < 0.01));
        assert!(mesh.vertices.iter().any(|v| v.uv.x > 0.99));
        assert!(
            mesh.vertices
                .iter()
                .all(|v| (0.0..=1.0).contains(&v.uv.x) && (0.0..=1.0).contains(&v.uv.y))
        );
        for corner in [
            clip.left_top(),
            clip.right_top(),
            clip.left_bottom(),
            clip.right_bottom(),
            clip.center(),
        ] {
            assert!(
                mesh.indices.chunks_exact(3).any(|triangle| {
                    let points = [triangle[0], triangle[1], triangle[2]]
                        .map(|i| mesh.vertices[i as usize].pos);
                    let crosses: Vec<_> = (0..3)
                        .map(|i| {
                            let a = points[i];
                            let b = points[(i + 1) % 3];
                            (b.x - a.x) * (corner.y - a.y) - (b.y - a.y) * (corner.x - a.x)
                        })
                        .collect();
                    crosses.iter().all(|&v| v >= -0.01) || crosses.iter().all(|&v| v <= 0.01)
                }),
                "local map covers {corner:?} at longitude {longitude}"
            );
        }
    }
}

#[test]
#[ignore = "requires a GPU and an explicit existing GEBCO terrain root"]
fn atlas_real_terrain_preview() {
    let root = PathBuf::from(std::env::var("ONEKEE_ATLAS_ROOT").expect("GEBCO asset root"));
    let mut gpu = screen::Screen::new([1536, 1024]);
    gpu.install_globe();
    let maximum = gpu.max_texture_side().min(8192);
    let region = super::super::atlas_viewport::Region([-8 * 3600, 37 * 3600, 24 * 3600, 56 * 3600]);
    let detail = super::super::atlas_source::load(Some(&root), region, maximum)
        .expect("native Europe terrain");
    let ctx = egui::Context::default();
    let initial = ctx.run(
        egui::RawInput {
            max_texture_side: Some(maximum),
            ..Default::default()
        },
        |_| {},
    );
    gpu.render(&ctx, initial, false);
    crate::theme::set_theme(&ctx, crate::theme::MapTheme::Atlas);
    let image = detail.grid.image_for(region.bounds()).unwrap();
    let texture = ctx.load_texture("atlas-preview", image, egui::TextureOptions::LINEAR);
    let mut model = crate::model::AppModel::new();
    model.map_theme = crate::theme::MapTheme::Atlas;
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1536., 1024.));
    let focus = GeoPoint { lat: 46., lon: 7.5 };
    let mut view = GlobeViewState::from_focus(focus);
    view.zoom = 17.;
    let layout = GlobeLayout {
        center: egui::pos2(975., 515.),
        radius: 1950.,
        focal_length: 2.8,
        camera_distance: 2.0,
    };
    for frame in 0..3 {
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(rect),
                max_texture_side: Some(maximum),
                ..Default::default()
            },
            |ctx| {
                egui::SidePanel::left("atlas-appearance")
                    .exact_width(330.)
                    .resizable(false)
                    .show(ctx, |ui| {
                        ui.heading("Appearance");
                        ui.add_space(20.);
                        appearance::render(ui, &mut model);
                    });
                egui::CentralPanel::default()
                    .frame(egui::Frame::new().fill(crate::theme::canvas_background()))
                    .show(ctx, |ui| {
                        let painter = ui.painter();
                        painter.add(
                            super::super::globe_pass::GlobeCallback::new(
                                layout.center,
                                layout.radius,
                                layout.focal_length,
                                layout.camera_distance,
                                view.yaw,
                                view.pitch,
                                1.,
                                false,
                                crate::model::ActiveBody::Earth,
                                crate::theme::scene_backdrop(),
                                crate::theme::topo_color(),
                                crate::theme::wireframe_color(),
                                crate::theme::grid_color(),
                                crate::theme::hot_color(),
                            )
                            .into_paint_callback(ui.max_rect()),
                        );
                        let mesh = super::super::raster_mesh::globe(
                            &layout,
                            &view,
                            texture.id(),
                            region.bounds(),
                            true,
                        );
                        painter.add(egui::Shape::mesh(mesh));
                    });
            },
        );
        if let Some(pixels) = gpu.render(&ctx, output, frame == 2) {
            assert!(
                pixels
                    .chunks_exact(4)
                    .filter(|p| p[2] > p[0].saturating_add(15))
                    .count()
                    > 1000,
                "visible blue ocean"
            );
            if let Ok(path) = std::env::var("ONEKEE_ATLAS_PREVIEW") {
                image::save_buffer(path, &pixels, 1536, 1024, image::ColorType::Rgba8).unwrap();
            }
        }
    }
    crate::theme::set_theme(&ctx, crate::theme::MapTheme::Topo);
}

#[test]
#[ignore = "requires installed GDAL and original GEBCO/SRTM terrain files"]
fn native_detail_publishes_in_both_scenes_without_blocking_draw() {
    use std::time::{Duration, Instant};
    let root = PathBuf::from(std::env::var("ONEKEE_ATLAS_ROOT").expect("existing terrain root"));
    let ctx = egui::Context::default();
    let region = super::super::atlas_viewport::Region([
        7 * 3600,
        46 * 3600,
        7 * 3600 + 900,
        46 * 3600 + 900,
    ]);
    clear();
    let start = Instant::now();
    assert!(detail_texture(&ctx, Some(&root), region, false).is_none());
    assert!(detail_texture(&ctx, Some(&root), region, true).is_none());
    assert!(
        start.elapsed() < Duration::from_millis(150),
        "first draw never waits for source I/O"
    );
    loop {
        let globe = detail_texture(&ctx, Some(&root), region, false);
        let local = detail_texture(&ctx, Some(&root), region, true);
        if let (Some(globe), Some(local)) = (globe, local) {
            assert_eq!(globe.texture.size(), [900, 900]);
            assert_eq!(local.texture.size(), [900, 900]);
            assert_eq!(globe.region, region);
            assert_eq!(local.region, region);
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(40));
        std::thread::sleep(Duration::from_millis(10));
    }
    clear();
}
