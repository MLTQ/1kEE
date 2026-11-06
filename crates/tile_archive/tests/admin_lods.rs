use cell_format::{CellFeature, CellPoint, TAG_ADMN, write::write_cell};
use std::{fs, path::Path};
use tile_archive::admin_lod::{Archive, ERRORS, FULL, Window, importance, simplify, unwrap};
fn feature(id: i64, polygon: bool, xy: &[(f32, f32)]) -> CellFeature {
    CellFeature {
        way_id: id,
        class: 2,
        is_polygon: polygon,
        name: None,
        points: xy
            .iter()
            .map(|&(lon, lat)| CellPoint { lon, lat })
            .collect(),
        elevations: None,
    }
}
fn publish(path: &Path, features: &[CellFeature]) {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, write_cell(0, 0, &[(TAG_ADMN, features)])).unwrap();
    fs::rename(tmp, path).unwrap();
}
#[test]
fn tiers_preserve_endpoints_nested_vertices_holes_and_original_detail() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("admin_level_2.1kc");
    let mut outer: Vec<_> = (0..=1000)
        .map(|i| (i as f32 / 100., 0.003 * (i as f32 * 0.14).sin()))
        .collect();
    outer.extend([(10., 10.), (0., 10.), outer[0]]);
    let mut coast = feature(10, true, &outer);
    coast.elevations = Some(vec![45.; coast.points.len()]);
    let hole = feature(
        10,
        true,
        &[(4., 4.), (6., 4.), (6., 6.), (4., 6.), (4., 4.)],
    );
    let island = feature(
        10,
        true,
        &[(12., 2.), (13., 2.), (13., 3.), (12., 3.), (12., 2.)],
    );
    publish(&path, &[coast.clone(), hole, island]);
    let mut a = Archive::open(&path, 2).unwrap();
    let counts = a.segment_counts().unwrap();
    assert!(counts.windows(2).all(|c| c[0] <= c[1]));
    assert!(counts[0] < counts[FULL] / 20);
    let mut previous = Vec::new();
    for tier in 0..=FULL {
        let mut shapes = Vec::new();
        a.query(tier, Window::WORLD, true, |p| shapes.push(p))
            .unwrap();
        assert_eq!(shapes.len(), 3);
        assert!(
            shapes
                .iter()
                .all(|p| p.complete && p.points.len() >= 4 && p.points.first() == p.points.last())
        );
        let mut finer = shapes[0].points.iter();
        for coarse in &previous {
            assert!(
                finer.any(|p| p == coarse),
                "LOD vertices must be nested and ordered"
            );
        }
        previous = shapes[0].points.clone();
        let mut enclosing = Vec::new();
        a.query(tier, Window::around([4.99, 4.99, 5.01, 5.01]), true, |p| {
            enclosing.push(p)
        })
        .unwrap();
        assert_eq!(
            enclosing.len(),
            2,
            "Interior queries keep both outer and hole, excluding the remote island"
        );
        if tier == FULL {
            assert_eq!(shapes[0].points, coast.points);
        }
    }
    // External replacement cannot change the source read by an already open index.
    publish(&path, &[feature(99, false, &[(0., 0.), (1., 1.)])]);
    a.query(FULL, Window::WORLD, true, |p| assert_eq!(p.relation, 10))
        .unwrap();
    Archive::open(&path, 2)
        .unwrap()
        .query(FULL, Window::WORLD, true, |p| {
            assert_eq!(p.relation, 99);
            assert!(!p.complete);
        })
        .unwrap();
    fs::write(&path, b"truncated replacement").unwrap();
    assert!(Archive::open(&path, 2).is_err());
}
#[test]
fn spatial_reads_crossings_dateline_and_all_members_of_incomplete_relations() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("admin_level_2.1kc");
    let crossing = feature(4, false, &[(179., 1.), (-179., 1.)]);
    let outer = feature(
        8,
        true,
        &[(178., 0.), (-178., 0.), (-178., 4.), (178., 4.), (178., 0.)],
    );
    let remote = feature(8, false, &[(20., 10.), (21., 10.)]);
    publish(&path, &[crossing, outer, remote]);
    let mut a = Archive::open(&path, 2).unwrap();
    for lon in [179.98, -180.02] {
        for detail in [0, FULL] {
            for shade in [false, true] {
                let mut pieces = Vec::new();
                a.query(
                    detail,
                    Window::around([lon, 0.98, lon + 0.02, 1.02]),
                    shade,
                    |p| pieces.push(p),
                )
                .unwrap();
                assert!(pieces.iter().all(|p| !p.complete));
                assert!(
                    pieces
                        .iter()
                        .any(|p| p.relation == 4 && p.points[1].lon - p.points[0].lon == 2.)
                );
            }
        }
    }
    a.query(FULL, Window::around([-1., -1., 1., 1.]), false, |_| {
        panic!("Offscreen geometry loaded")
    })
    .unwrap();
}
#[test]
fn each_lod_stays_within_its_error_of_the_open_source_line() {
    let mut points: Vec<_> = (0..300)
        .map(|i| CellPoint {
            lon: (i as f32 / 100. + 179. + 180.).rem_euclid(360.) - 180.,
            lat: (i as f32 * 0.1).sin() * 0.08,
        })
        .collect();
    let anchor = points[0].lon;
    unwrap(&mut points, anchor);
    let weights = importance(&points);
    for (tier, tolerance) in ERRORS.iter().enumerate() {
        let line = simplify(&points, &weights, tier);
        assert_eq!(line.first(), points.first());
        assert_eq!(line.last(), points.last());
        for p in &points {
            let distance = line
                .windows(2)
                .map(|pair| {
                    let (a, b) = (pair[0], pair[1]);
                    let (dx, dy) = (b.lon - a.lon, b.lat - a.lat);
                    let (x, y) = (p.lon - a.lon, p.lat - a.lat);
                    let t = ((x * dx + y * dy) / (dx * dx + dy * dy)).clamp(0., 1.);
                    (x - t * dx).hypot(y - t * dy)
                })
                .fold(f32::INFINITY, f32::min);
            assert!(distance <= tolerance + 0.00001);
        }
    }
}
