use super::*;

#[test]
fn missing_heights_are_transparent_without_affecting_neighbor_pigments() {
    let mut grid = HeightGrid {
        width: 5,
        height: 5,
        metres: vec![200.; 25],
    };
    let flat = grid.image().unwrap();
    grid.metres[11] = -32767.;
    grid.metres[13] = f32::NAN;
    let masked = grid.image().unwrap();
    assert_eq!(masked.pixels[11], Color32::TRANSPARENT);
    assert_eq!(masked.pixels[13], Color32::TRANSPARENT);
    assert_eq!(masked.pixels[12], flat.pixels[12]);
    grid.metres.pop();
    assert!(grid.image().is_none());
}

#[test]
fn pigments_have_red_mid_elevations_pale_plains_and_deep_blue_water() {
    let grid = HeightGrid {
        width: 8,
        height: 4,
        metres: vec![-5000., -1500., -200., 0., 200., 1200., 2000., 5500.].repeat(4),
    };
    let image = grid.image().unwrap();
    let c = &image.pixels;
    assert!(c[0].b() > c[0].r() * 2 && c[0].b() < c[2].b());
    assert!(c[3].r() > 230 && c[3].g() > 220);
    assert!(c[5].r() > c[5].g() + 60);
    assert!(c[6].r() > c[6].g() * 2);
    assert!(c[7].r() > 230 && c[7].b() > 190);
    assert!(c.iter().all(|c| c.a() == 255));
}

#[test]
fn colors_have_no_baked_direction_or_scale_dependent_shadow() {
    let mut grid = HeightGrid {
        width: 48,
        height: 48,
        metres: vec![1000.; 48 * 48],
    };
    let i = 24 * 48 + 24;
    let original = grid.image().unwrap().pixels[i];
    grid.metres[18 * 48 + 18] = 4000.;
    grid.metres[i - 1] = 0.;
    grid.metres[i + 1] = 3000.;
    assert_eq!(grid.image().unwrap().pixels[i], original);
    assert_eq!(
        grid.image_for([7., 46., 7.15, 46.15]).unwrap().pixels[i],
        original
    );
    assert!(grid.image_for([7., 46., 7., 47.]).is_none());
}
