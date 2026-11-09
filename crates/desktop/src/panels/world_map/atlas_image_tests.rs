use super::*;

#[test]
fn missing_heights_are_transparent_and_cannot_create_cliffs() {
    let mut grid = HeightGrid {
        width: 5,
        height: 5,
        metres: vec![200.0; 25],
    };
    let flat = grid.image().unwrap();
    grid.metres[11] = -32767.0;
    grid.metres[13] = f32::NAN;
    let masked = grid.image().unwrap();
    assert_eq!(masked.pixels[11], Color32::TRANSPARENT);
    assert_eq!(masked.pixels[13], Color32::TRANSPARENT);
    assert_eq!(
        masked.pixels[12], flat.pixels[12],
        "missing neighbours never cast a shadow"
    );
    grid.metres.pop();
    assert!(grid.image().is_none());
}

#[test]
fn water_and_land_use_distinct_tints_with_directional_relief() {
    let grid = HeightGrid {
        width: 8,
        height: 4,
        metres: vec![-4000.0, -4000.0, -200.0, -1.0, 0.0, 650.0, 2000.0, 5500.0].repeat(4),
    };
    let image = grid.image().unwrap();
    let ocean = image.pixels[8];
    let land = image.pixels[12];
    assert!(ocean.b() > ocean.r());
    assert!(land.r() > land.b());
    assert!(image.pixels.iter().all(|c| c.a() == 255));
    let base = HeightGrid {
        width: 720,
        height: 360,
        metres: vec![1000.0; 720 * 360],
    };
    let mut west_lit = HeightGrid {
        metres: base.metres.clone(),
        ..base
    };
    let center = 180 * 720 + 360;
    west_lit.metres[center - 1] = 0.0;
    west_lit.metres[center + 1] = 2000.0;
    let lit = west_lit.image().unwrap().pixels[center];
    west_lit.metres[center - 1] = 2000.0;
    west_lit.metres[center + 1] = 0.0;
    let shaded = west_lit.image().unwrap().pixels[center];
    assert!(
        lit.r() > shaded.r(),
        "west-facing slopes receive northwest light"
    );
}

#[test]
fn slope_samples_wrap_at_the_dateline() {
    let mut grid = HeightGrid {
        width: 720,
        height: 360,
        metres: vec![0.0; 720 * 360],
    };
    let row = 180 * 720;
    grid.metres[row] = 1000.0;
    grid.metres[row + 1] = 2000.0;
    grid.metres[row + 360] = 1000.0;
    grid.metres[row + 361] = 2000.0;
    let image = grid.image().unwrap();
    assert_eq!(image.pixels[row], image.pixels[row + 360]);
}

#[test]
fn measured_ridge_casts_a_shadow_at_regional_scale() {
    let mut grid = HeightGrid {
        width: 48,
        height: 48,
        metres: vec![1000.0; 48 * 48],
    };
    let bounds = [7.0, 46.0, 7.15, 46.15];
    let center = 24 * 48 + 24;
    let flat = grid.image_for(bounds).unwrap().pixels[center];
    // Only the northwest horizon changes; the central-difference normals
    // around this valley point remain identical to the flat reference.
    grid.metres[18 * 48 + 18] = 4000.0;
    let shaded = grid.image_for(bounds).unwrap().pixels[center];
    assert!(
        shaded.r() + 30 < flat.r(),
        "the measured northwest ridge casts a visible shadow"
    );
    let world = grid.image().unwrap().pixels[center];
    assert!(
        world.r() > shaded.r(),
        "regional spacing is not mistaken for a world-sized raster"
    );
}
