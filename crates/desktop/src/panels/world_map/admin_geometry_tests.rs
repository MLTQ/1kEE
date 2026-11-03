use super::*;
fn ring(points: &[(f32, f32)]) -> Vec<GeoPoint> {
    points
        .iter()
        .map(|&(lon, lat)| GeoPoint { lon, lat })
        .collect()
}
fn area(fill: &Fill) -> f32 {
    fill.indices
        .chunks_exact(3)
        .map(|t| {
            let (a, b, c) = (
                fill.vertices[t[0] as usize],
                fill.vertices[t[1] as usize],
                fill.vertices[t[2] as usize],
            );
            ((b.lon - a.lon) * (c.lat - a.lat) - (b.lat - a.lat) * (c.lon - a.lon)).abs() * 0.5
        })
        .sum()
}
#[test]
fn admin_fill_preserves_concavity_holes_and_islands() {
    let rings = vec![
        ring(&[
            (0., 0.),
            (6., 0.),
            (6., 2.),
            (4., 2.),
            (4., 6.),
            (0., 6.),
            (0., 0.),
        ]),
        ring(&[(1., 1.), (2., 1.), (2., 2.), (1., 2.), (1., 1.)]),
        ring(&[(8., 0.), (9., 0.), (9., 1.), (8., 1.), (8., 0.)]),
    ];
    assert!((area(&tessellate(&rings).unwrap()) - 28.).abs() < 1e-5);
}
#[test]
fn admin_fill_does_not_close_incomplete_boundaries() {
    assert!(tessellate(&[ring(&[(0., 0.), (2., 0.), (2., 2.)])]).is_err());
}
#[test]
fn admin_dateline_region_has_small_area() {
    let mut rings = vec![ring(&[
        (179., 0.),
        (181., 0.),
        (181., 2.),
        (179., 2.),
        (179., 0.),
    ])];
    // A hole starts in the other longitude copy (the loader unwraps each ring).
    rings.push(ring(&[
        (-179.75, 0.5),
        (-179.25, 0.5),
        (-179.25, 1.5),
        (-179.75, 1.5),
        (-179.75, 0.5),
    ]));
    align_rings(&mut rings);
    assert!((area(&tessellate(&rings).unwrap()) - 3.5).abs() < 1e-5);
}
