//! Moving the light must change shadows, never terrain or its upload identity.
use super::*;

#[test]
#[ignore = "requires GPU; verifies movable lighting, real cast shadows and pigment separation"]
fn moving_light_reverses_measured_shadows_without_changing_geometry() {
    use crate::atlas_style::AtlasStyle;
    let mut gpu = screen::Screen::new([1024, 768]);
    let ctx = egui::Context::default();
    let data = raster(
        [7.1, 46.1, 7.9, 46.9],
        |x, _| 200.0 + 3000.0 * (-((x - 0.42) / 0.015).powi(2)).exp(),
        [200, 140, 105, 255],
    );
    let source_id = data.id;
    let west = AtlasStyle {
        azimuth_deg: 270.0,
        altitude_deg: 20.0,
        ..Default::default()
    };
    let east = AtlasStyle {
        azimuth_deg: 90.0,
        ..west
    };
    let shadow = render_styled(&mut gpu, &ctx, vec![data.clone()], false, 3.0, west);
    let lit = render_styled(&mut gpu, &ctx, vec![data.clone()], false, 3.0, east);
    assert!(uploaded(&ctx, 1, source_id));
    let background = &shadow[..4];
    let mut changed = 0;
    for (a, b) in shadow.chunks_exact(4).zip(lit.chunks_exact(4)) {
        assert_eq!(
            a == background,
            b == background,
            "light cannot alter terrain coverage"
        );
        if a != b {
            changed += 1;
        }
    }
    assert!(changed > 5000, "moving light updates retained terrain");
    // This flat valley is 8 km east of the ridge: its fine/broad normal and
    // pigment are unchanged. Only the measured western horizon blocks the light.
    let layout = LocalLayout {
        center: egui::pos2(512., 384.),
        focus_center: egui::pos2(512., 384.),
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
    view.local_layer_spread = 3.;
    let p = local_projection_params(
        &layout,
        &view,
        focus,
        0.55 * 111.32 * focus.lat.to_radians().cos(),
        0.55 * 111.32,
    );
    let (x, y, _) = p.project_logical(
        GeoPoint {
            lat: 46.5,
            lon: 7.54,
        },
        200.,
    );
    let index = (y.round() as usize * 1024 + x.round() as usize) * 4;
    let brightness = |pixels: &[u8]| {
        pixels[index..index + 3]
            .iter()
            .map(|&v| v as u32)
            .sum::<u32>()
    };
    assert!(
        brightness(&lit) > brightness(&shadow) + 60,
        "the ridge casts a real movable shadow"
    );
    let unlit = AtlasStyle {
        relief_strength: 0.0,
        ..west
    };
    let a = render_styled(&mut gpu, &ctx, vec![data.clone()], false, 3., unlit);
    let b = render_styled(
        &mut gpu,
        &ctx,
        vec![data.clone()],
        false,
        3.,
        AtlasStyle {
            azimuth_deg: 90.,
            ..unlit
        },
    );
    assert_eq!(a, b, "pigments contain no fixed lighting");
    let daylight = render_styled(
        &mut gpu,
        &ctx,
        vec![data.clone()],
        false,
        3.,
        AtlasStyle {
            light_mode: crate::atlas_style::LightMode::Sun,
            ..Default::default()
        },
    );
    let night = render_styled(
        &mut gpu,
        &ctx,
        vec![data.clone()],
        false,
        3.,
        AtlasStyle {
            light_mode: crate::atlas_style::LightMode::Sun,
            time_offset_hours: 12.,
            ..Default::default()
        },
    );
    assert_ne!(
        daylight, night,
        "Sun time offset changes terrain illumination"
    );
    let gray = render_styled(
        &mut gpu,
        &ctx,
        vec![data],
        false,
        3.,
        AtlasStyle {
            color_richness: 0.,
            ..west
        },
    );
    let terrain_pixels = gray.chunks_exact(4).filter(|p| *p != background).count();
    assert!(terrain_pixels > 5000);
    let max_chroma = gray
        .chunks_exact(4)
        .filter(|p| *p != background)
        .map(|p| {
            p[0].abs_diff(p[1])
                .max(p[1].abs_diff(p[2]))
                .max(p[0].abs_diff(p[2]))
        })
        .max()
        .unwrap();
    assert!(
        max_chroma <= 1,
        "grayscale allows one byte of GPU rounding, got {max_chroma}"
    );
}
