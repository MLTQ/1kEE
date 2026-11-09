use super::*;
#[path = "atlas_light_gpu_tests.rs"]
mod lighting_tests;
#[path = "admin_test_screen.rs"]
mod screen;

fn raster(bounds: [f32; 4], height: impl Fn(f32, f32) -> f32, color: [u8; 4]) -> Arc<Raster> {
    let n = 129;
    let mut data = Raster::new(
        HeightGrid {
            width: n,
            height: n,
            metres: (0..n)
                .flat_map(|y| (0..n).map(move |x| (x, y)))
                .map(|(x, y)| height(x as f32 / (n - 1) as f32, y as f32 / (n - 1) as f32))
                .collect(),
        },
        bounds,
    )
    .unwrap();
    Arc::get_mut(&mut data).unwrap().colors = color.repeat(n * n);
    data
}
fn render(
    gpu: &mut screen::Screen,
    ctx: &egui::Context,
    rasters: Vec<Arc<Raster>>,
    contours: bool,
    spread: f32,
) -> Vec<u8> {
    render_styled(
        gpu,
        ctx,
        rasters,
        contours,
        spread,
        crate::atlas_style::AtlasStyle::default(),
    )
}
fn render_styled(
    gpu: &mut screen::Screen,
    ctx: &egui::Context,
    rasters: Vec<Arc<Raster>>,
    contours: bool,
    spread: f32,
    style: crate::atlas_style::AtlasStyle,
) -> Vec<u8> {
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1024., 768.));
    let layout = LocalLayout {
        center: rect.center(),
        focus_center: rect.center(),
        width: 840.,
        height: 568.,
        horizontal_scale: 317.,
    };
    let focus = GeoPoint {
        lat: 46.5,
        lon: 7.5,
    };
    let mut view = GlobeViewState::from_focus(focus);
    view.local_yaw = 0.;
    view.local_pitch = 1.3;
    view.local_layer_spread = spread;
    let wanted = rasters.last().unwrap().id;
    for _ in 0..20 {
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(rect),
                ..Default::default()
            },
            |ctx| {
                let painter = ctx
                    .layer_painter(egui::LayerId::background())
                    .with_clip_rect(rect);
                let mut callback = Callback::local(
                    &painter,
                    &layout,
                    &view,
                    rasters[0].clone(),
                    focus,
                    0.55,
                    contours,
                    false,
                    1.0,
                );
                for raster in rasters.iter().skip(1) {
                    let next = Callback::local(
                        &painter,
                        &layout,
                        &view,
                        raster.clone(),
                        focus,
                        0.55,
                        contours,
                        false,
                        1.0,
                    );
                    callback.data.extend(next.data);
                    callback.uniforms.extend(next.uniforms);
                }
                callback
                    .with_style(style, crate::stellar_time::JD_J2000)
                    .submit(&painter);
            },
        );
        let pixels = gpu.render(ctx, output, true).unwrap();
        if uploaded(ctx, 1, wanted) {
            return pixels;
        }
    }
    panic!("private terrain upload never completed");
}
#[test]
#[ignore = "requires GPU; proves real displacement, attached contours and depth occlusion"]
fn height_geometry_contours_and_occlusion_are_real() {
    let mut gpu = screen::Screen::new([1024, 768]);
    let ctx = egui::Context::default();
    let bounds = [7.1, 46.1, 7.9, 46.9];
    let pigment = [177, 143, 109, 255];
    let flat = raster(bounds, |_, _| 0., pigment);
    let raised = raster(
        bounds,
        |x, y| 100. + 3500. * (-((x - 0.5).powi(2) + (y - 0.5).powi(2)) / 0.035).exp(),
        pigment,
    );
    let flat_pixels = render(&mut gpu, &ctx, vec![flat], false, 3.);
    let raised_pixels = render(&mut gpu, &ctx, vec![raised.clone()], false, 3.);
    let background = &flat_pixels[..4];
    let added = flat_pixels
        .chunks_exact(4)
        .zip(raised_pixels.chunks_exact(4))
        .filter(|(a, b)| *a == background && *b != background)
        .count();
    assert!(
        added > 2000,
        "measured height changes the silhouette with identical pigments"
    );
    let lined = render(&mut gpu, &ctx, vec![raised.clone()], true, 3.);
    assert_ne!(
        lined, raised_pixels,
        "contours toggle on the actual terrain surface"
    );
    for (a, b) in lined.chunks_exact(4).zip(raised_pixels.chunks_exact(4)) {
        assert_eq!(
            a == background,
            b == background,
            "no contour fragments float outside the surface"
        );
    }
    let taller = render(&mut gpu, &ctx, vec![raised], true, 8.);
    assert_ne!(
        taller, lined,
        "layer spread changes the 3D surface and its contours together"
    );
    let near = raster([7.15, 46.05, 7.85, 46.45], |_, _| 3000., [200, 80, 55, 255]);
    let far = raster([7.15, 46.55, 7.85, 46.95], |_, _| 0., [50, 100, 180, 255]);
    let a = render(&mut gpu, &ctx, vec![near.clone()], false, 3.);
    let b = render(&mut gpu, &ctx, vec![far.clone()], false, 3.);
    assert!(
        a.chunks_exact(4)
            .zip(b.chunks_exact(4))
            .filter(|(a, b)| *a != background && *b != background)
            .count()
            > 100,
        "occlusion fixture overlaps"
    );
    let ab = render(&mut gpu, &ctx, vec![near.clone(), far.clone()], false, 3.);
    let ba = render(&mut gpu, &ctx, vec![far, near], false, 3.);
    assert_eq!(ab, ba, "depth visibility is independent of draw order");
    if let Ok(path) = std::env::var("ONEKEE_ATLAS_3D_FIXTURE") {
        image::save_buffer(path, &lined, 1024, 768, image::ColorType::Rgba8).unwrap();
    }
}

#[test]
#[ignore = "requires GPU, GDAL and original SRTM terrain"]
fn native_alps_are_a_displaced_surface() {
    let root = std::path::PathBuf::from(std::env::var("ONEKEE_ATLAS_ROOT").expect("terrain root"));
    let region =
        super::super::super::atlas_viewport::Region([7 * 3600, 46 * 3600, 8 * 3600, 47 * 3600]);
    let detail = super::super::super::atlas_source::load(Some(&root), region, 4096).unwrap();
    let raster = Raster::new(detail.grid, region.bounds()).unwrap();
    let mut gpu = screen::Screen::new([1024, 768]);
    let ctx = egui::Context::default();
    crate::theme::set_theme(&ctx, crate::theme::MapTheme::Atlas);
    let pixels = render(&mut gpu, &ctx, vec![raster], true, 3.);
    if let Ok(path) = std::env::var("ONEKEE_ATLAS_3D_PREVIEW") {
        image::save_buffer(path, &pixels, 1024, 768, image::ColorType::Rgba8).unwrap();
    }
    crate::theme::set_theme(&ctx, crate::theme::MapTheme::Topo);
}
