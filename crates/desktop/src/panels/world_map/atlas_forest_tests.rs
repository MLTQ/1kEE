use super::*;
fn points(values: &[(f32, f32)]) -> Vec<GeoPoint> {
    values
        .iter()
        .map(|&(lon, lat)| GeoPoint { lon, lat })
        .collect()
}
#[test]
fn concave_coverage_keeps_the_bay_and_duplicate_cells_do_not_darken_it() {
    let ring = points(&[
        (0., 0.),
        (4., 0.),
        (4., 4.),
        (3., 4.),
        (3., 1.),
        (1., 1.),
        (1., 4.),
        (0., 4.),
        (0., 0.),
    ]);
    let mut mask = vec![0; 64];
    paint_ring(&mut mask, [8, 8], [0., 0., 4., 4.], &ring);
    assert_eq!(mask.iter().filter(|&&v| v == 255).count(), 40);
    assert_eq!(mask[2 * 8 + 4], 0);
    let original = mask.clone();
    paint_ring(&mut mask, [8, 8], [0., 0., 4., 4.], &ring);
    assert_eq!(mask, original);
}
#[test]
fn invalid_and_open_forests_cannot_invent_coverage() {
    let mut ring = points(&[(0., 0.), (2., 0.), (2., 2.)]);
    let mut mask = vec![0; 64];
    paint_ring(&mut mask, [8, 8], [0., 0., 4., 4.], &ring);
    ring.push(ring[0]);
    ring[1].lat = f32::NAN;
    paint_ring(&mut mask, [8, 8], [0., 0., 4., 4.], &ring);
    assert!(mask.iter().all(|&v| v == 0));
}
#[test]
fn date_line_coverage_and_offscreen_clipping_are_geographic() {
    let ring = points(&[
        (179.9, 0.),
        (-179.9, 0.),
        (-179.9, 0.2),
        (179.9, 0.2),
        (179.9, 0.),
    ]);
    let mut mask = vec![0; 64];
    paint_ring(&mut mask, [8, 8], [179.8, -0.1, 180.2, 0.3], &ring);
    assert_eq!(mask.iter().filter(|&&v| v == 255).count(), 16);
    let ring = points(&[
        (-100., -100.),
        (100., -100.),
        (100., 100.),
        (-100., 100.),
        (-100., -100.),
    ]);
    // A region wider than 180 degrees is not a normal forest; use local outside edges.
    let ring = ring
        .iter()
        .map(|p| GeoPoint {
            lon: p.lon * 0.1,
            lat: p.lat * 0.1,
        })
        .collect::<Vec<_>>();
    paint_ring(&mut mask, [8, 8], [0., 0., 4., 4.], &ring);
    assert!(mask.iter().all(|&v| v == 255));
}

#[test]
fn source_cell_limits_are_half_open_without_losing_a_strip_of_geometry() {
    for cell in [-180, -1, 0, 9, 46, 179] {
        let end = cell_end(cell);
        assert_eq!(end.floor() as i32, cell);
        assert!(end < (cell + 1) as f32);
        let adjacent = if end < 0.0 {
            f32::from_bits(end.to_bits() - 1)
        } else {
            f32::from_bits(end.to_bits() + 1)
        };
        assert_eq!(adjacent, (cell + 1) as f32);
    }
}
