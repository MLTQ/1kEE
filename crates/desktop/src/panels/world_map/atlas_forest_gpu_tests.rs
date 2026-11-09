use super::*;
#[test]
#[ignore = "requires GPU; forest coverage must share terrain geometry, depth and lighting"]
fn forest_material_follows_the_same_surface_and_visibility_restores_it() {
    use super::super::super::super::atlas_forest::Mask;
    use crate::atlas_style::AtlasStyle;
    let mut gpu = screen::Screen::new([1024, 768]);
    let ctx = egui::Context::default();
    let data = raster(
        [7.1, 46.1, 7.9, 46.9],
        |x, y| 200. + 3000. * (-((x - 0.5).powi(2) + (y - 0.5).powi(2)) / 0.035).exp(),
        [190, 120, 80, 255],
    );
    let forest = Arc::new(Mask::new(
        data.id,
        data.size,
        vec![255; data.size[0] * data.size[1]],
    ));
    let style = AtlasStyle::default();
    let baseline = render_styled(&mut gpu, &ctx, vec![data.clone()], false, 8., style);
    let covered = render_material(
        &mut gpu,
        &ctx,
        vec![data.clone()],
        false,
        8.,
        style,
        Some(forest.clone()),
    );
    assert_ne!(covered, baseline);
    let background = &baseline[..4];
    for (a, b) in baseline.chunks_exact(4).zip(covered.chunks_exact(4)) {
        assert_eq!(
            a == background,
            b == background,
            "forest cannot float outside terrain or bypass its visibility"
        );
    }
    let moved = render_material(
        &mut gpu,
        &ctx,
        vec![data.clone()],
        false,
        8.,
        AtlasStyle {
            azimuth_deg: 135.,
            ..style
        },
        Some(forest),
    );
    assert_ne!(
        covered, moved,
        "forest receives the same movable terrain light"
    );
    let restored = render_styled(&mut gpu, &ctx, vec![data.clone()], false, 8., style);
    assert_eq!(
        restored, baseline,
        "no baked or stale forest remains after hiding the layer"
    );
    let wrong = Arc::new(Mask::new(
        data.id + 100,
        data.size,
        vec![255; data.size[0] * data.size[1]],
    ));
    let ignored = render_material(&mut gpu, &ctx, vec![data], false, 8., style, Some(wrong));
    assert_eq!(
        ignored, baseline,
        "a different height-field mask cannot be painted on this source"
    );
}
