use super::*;

fn polyline(coords: &[(f32, f32)]) -> LoadedPolyline {
    LoadedPolyline {
        way_id: 1,
        class: "forest".into(),
        name: None,
        points: coords
            .iter()
            .map(|&(lon, lat)| GeoPoint { lon, lat })
            .collect(),
        elevations: Some(
            coords
                .iter()
                .map(|&(x, y)| 100.0 + 20.0 * x + 30.0 * y)
                .collect(),
        ),
        is_polygon: true,
    }
}
fn area(fill: &geometry::Fill) -> f64 {
    fill.indices
        .chunks_exact(3)
        .map(|t| {
            let [a, b, c] = [
                fill.vertices[t[0] as usize].0,
                fill.vertices[t[1] as usize].0,
                fill.vertices[t[2] as usize].0,
            ];
            (((b.lon - a.lon) as f64 * (c.lat - a.lat) as f64)
                - ((c.lon - a.lon) as f64 * (b.lat - a.lat) as f64))
                .abs()
                * 0.5
        })
        .sum()
}

#[test]
fn concave_forest_keeps_its_bay_and_baked_heights() {
    let poly = polyline(&[
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
    let feature = ElevatedTree::from_polyline(&poly, None);
    let fill = feature.fill.as_ref().unwrap();
    assert!(
        (area(fill) - 10.0).abs() < 0.0001,
        "concave bay is not filled by a fan"
    );
    for &(p, h) in &fill.vertices {
        assert!(
            (h - (100.5 + 20. * p.lon + 30. * p.lat)).abs() < 0.001,
            "baked heights stay attached to vertices"
        );
    }
    for t in fill.indices.chunks_exact(3) {
        let x = t
            .iter()
            .map(|&i| fill.vertices[i as usize].0.lon)
            .sum::<f32>()
            / 3.;
        let y = t
            .iter()
            .map(|&i| fill.vertices[i as usize].0.lat)
            .sum::<f32>()
            / 3.;
        assert!(
            !(x > 1. && x < 3. && y > 1.),
            "triangles stay out of the forest's bay"
        );
    }
}

#[test]
fn crossings_interpolate_elevation_and_open_rings_are_not_closed() {
    let poly = polyline(&[(0., 0.), (2., 2.), (0., 2.), (2., 0.), (0., 0.)]);
    let feature = ElevatedTree::from_polyline(&poly, None);
    let fill = feature.fill.as_ref().unwrap();
    let intersection = fill
        .vertices
        .iter()
        .find(|(p, _)| (p.lon - 1.).abs() < 0.0001 && (p.lat - 1.).abs() < 0.0001)
        .unwrap();
    assert!((intersection.1 - 150.5).abs() < 0.001);
    assert!((area(fill) - 2.).abs() < 0.0001);
    let mut open = polyline(&[(0., 0.), (1., 0.), (1., 1.)]);
    assert!(ElevatedTree::from_polyline(&open, None).fill.is_none());
    open.points[1].lat = f32::NAN;
    assert!(ElevatedTree::from_polyline(&open, None).fill.is_none());
}

#[test]
fn projected_hairpin_outlines_never_make_unbounded_corner_spikes() {
    // These are legal screen positions for a height-projected boundary folding
    // almost back on itself. The old filled closed PathShape extruded its miter
    // far across the canvas at the second point.
    let points = vec![
        (
            GeoPoint {
                lon: 100.,
                lat: 100.,
            },
            0.,
        ),
        (
            GeoPoint {
                lon: 900.,
                lat: 100.,
            },
            0.,
        ),
        (
            GeoPoint {
                lon: 100.,
                lat: 100.001,
            },
            0.,
        ),
        (
            GeoPoint {
                lon: 100.,
                lat: 500.,
            },
            0.,
        ),
        (
            GeoPoint {
                lon: 100.,
                lat: 100.,
            },
            0.,
        ),
    ];
    let rect = egui::Rect::from_min_max(egui::pos2(0., 0.), egui::pos2(1024., 768.));
    let ctx = egui::Context::default();
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(rect),
            ..Default::default()
        },
        |ctx| {
            let painter = ctx.layer_painter(egui::LayerId::background());
            painter.extend(geometry::outlines(
                &points,
                egui::Stroke::new(0.6, egui::Color32::GREEN),
                |p, _| Some(egui::pos2(p.lon, p.lat)),
            ));
        },
    );
    let bounds =
        egui::Rect::from_min_max(egui::pos2(100., 100.), egui::pos2(900., 500.)).expand(2.);
    let mut vertices = 0;
    for primitive in ctx.tessellate(output.shapes, output.pixels_per_point) {
        if let egui::epaint::Primitive::Mesh(mesh) = primitive.primitive {
            for vertex in mesh.vertices {
                assert!(vertex.pos.is_finite() && bounds.contains(vertex.pos));
                vertices += 1;
            }
        }
    }
    assert!(vertices > 10);
}

#[test]
fn missing_projections_do_not_bridge_unrelated_boundary_points() {
    let points = (0..5)
        .map(|i| {
            (
                GeoPoint {
                    lon: i as f32,
                    lat: 0.,
                },
                0.,
            )
        })
        .collect::<Vec<_>>();
    let shapes = geometry::outlines(
        &points,
        egui::Stroke::new(1., egui::Color32::GREEN),
        |p, _| (p.lon != 2.).then_some(egui::pos2(p.lon, 0.)),
    );
    assert_eq!(shapes.len(), 2);
    for shape in shapes {
        let egui::Shape::Path(path) = shape else {
            panic!("expected an open stroke")
        };
        assert!(!path.closed && path.fill == egui::Color32::TRANSPARENT);
        assert!(
            path.points
                .iter()
                .map(|p| p.x)
                .fold(f32::NEG_INFINITY, f32::max)
                - path
                    .points
                    .iter()
                    .map(|p| p.x)
                    .fold(f32::INFINITY, f32::min)
                <= 1.
        );
    }
}

#[test]
fn date_line_forests_keep_short_edges() {
    let mut p = polyline(&[
        (179.9, 0.),
        (-179.9, 0.),
        (-179.9, 0.2),
        (179.9, 0.2),
        (179.9, 0.),
    ]);
    p.elevations = Some(vec![100.; 5]);
    let feature = ElevatedTree::from_polyline(&p, None);
    assert!(
        feature
            .points
            .windows(2)
            .all(|p| (p[0].0.lon - p[1].0.lon).abs() < 1.)
    );
    assert!((area(feature.fill.as_ref().unwrap()) - 0.04).abs() < 0.00001);
}

#[path = "tree_runtime_tests.rs"]
mod runtime;
