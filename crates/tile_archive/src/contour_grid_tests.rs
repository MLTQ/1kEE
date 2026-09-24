use super::*;

#[test]
fn neighboring_cores_have_identical_edges_and_aligned_halo_samples() {
    for (h, n) in [(3.6, 384), (0.16, 896), (0.0148, 2400)] {
        for y in [-8000, 0, 6360] {
            for x in [-11000, 0, 9000] {
                let a = CoreTile::new(h, n, y, x);
                let b = CoreTile::new(h, n, y, x + 1);
                let north = CoreTile::new(h, n, y + 1, x);
                assert_eq!(a.core.max_lon, b.core.min_lon);
                assert_eq!(a.core.max_lat, north.core.min_lat);
                let da = (a.source.max_lon - a.source.min_lon) / f64::from(a.raster_size);
                let db = (b.source.max_lon - b.source.min_lon) / f64::from(b.raster_size);
                for i in 0..4 {
                    let pa = a.source.min_lon + (f64::from(a.raster_size - 4 + i) + 0.5) * da;
                    let pb = b.source.min_lon + (f64::from(i) + 0.5) * db;
                    assert!((pa - pb).abs() < 1e-10);
                }
                assert!(f64::from(a.raster_size).powi(2) / f64::from(n).powi(2) < 0.057);
            }
        }
    }
}
#[test]
fn clipping_preserves_crossings_without_bridging_outside_excursions() {
    let b = Bounds {
        min_lon: 0.,
        max_lon: 1.,
        min_lat: 0.,
        max_lat: 1.,
    };
    assert_eq!(
        clip_line(&[(-1., 0.5), (2., 0.5)], b),
        vec![vec![(0., 0.5), (1., 0.5)]]
    );
    let paths = clip_line(&[(0.5, 0.2), (2., 0.2), (2., 0.8), (0.5, 0.8)], b);
    assert_eq!(
        paths,
        vec![vec![(0.5, 0.2), (1., 0.2)], vec![(1., 0.8), (0.5, 0.8)]]
    );
    assert!(clip_line(&[(1., 0.), (1., 1.)], b).is_empty());
    assert_eq!(clip_line(&[(0., 0.), (0., 1.)], b).len(), 1);
    assert!(clip_line(&[(f64::NAN, 0.), (0.5, 0.5)], b).is_empty());
}
#[test]
fn adjacent_clips_join_at_the_same_endpoint() {
    let a = CoreTile::new(0.0148, 2400, 6360, -10660).core;
    let b = CoreTile::new(0.0148, 2400, 6360, -10659).core;
    let cy = (a.min_lat + a.max_lat) / 2.;
    let line = vec![
        (a.min_lon - 0.001, cy - 0.001),
        (b.max_lon + 0.001, cy + 0.001),
    ];
    let left = clip_line(&line, a);
    let right = clip_line(&line, b);
    assert_eq!(left[0].last(), right[0].first());
}
#[test]
fn clipped_gpkg_is_readable_by_legacy_and_packed_readers() {
    let b = Bounds {
        min_lon: 0.,
        max_lon: 1.,
        min_lat: 0.,
        max_lat: 1.,
    };
    let blob = crate::contour_clip::encode(&[vec![(-1., 0.5), (2., 0.5)]]);
    let clipped = crate::contour_clip::clip_gpkg(&blob, b).unwrap().unwrap();
    let expected = vec![vec![(0f32, 0.5f32), (1., 0.5)]];
    assert_eq!(
        crate::gpkg::parse_gpkg_lines(&clipped, |x, y| (x, y)),
        expected
    );
    let payload = crate::contours::encode(&[(123., clipped)]).unwrap();
    let decoded = crate::contours::decode(&payload).unwrap();
    assert_eq!(decoded[0].0, 123.);
    assert_eq!(
        crate::contours::decode_lines(decoded[0].1, |x, y| (x, y)).unwrap(),
        expected
    );
    for end in 0..blob.len() {
        assert!(crate::contour_clip::clip_gpkg(&blob[..end], b).is_err());
    }
}

#[test]
#[ignore = "read-only: set ONEKEE_CONTOUR_BENCH_DB to measure selected real tiles"]
fn benchmark_real_core_storage() {
    let path = std::env::var("ONEKEE_CONTOUR_BENCH_DB").unwrap();
    let db =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let keys = [(10, 5317, -12463), (10, 6360, -10670), (10, 6364, -10670)];
    for (z, y, x) in keys {
        let core = CoreTile::new(0.0148, 2400, y, x).core;
        let mut q=db.prepare("SELECT geom FROM contour_tiles WHERE zoom_bucket=?1 AND lat_bucket=?2 AND lon_bucket=?3").unwrap();
        let mut rows = q.query(rusqlite::params![z, y, x]).unwrap();
        let (mut old, mut new, mut kept) = (0, 0, 0);
        while let Some(row) = rows.next().unwrap() {
            let blob = row.get_ref(0).unwrap().as_blob().unwrap();
            old += blob.len();
            if let Some(out) = crate::contour_clip::clip_gpkg(blob, core).unwrap() {
                new += out.len();
                kept += 1;
            }
        }
        assert!(old > 0, "missing sample {z}/{y}/{x}");
        eprintln!(
            "core sample {z}/{y}/{x}: {old} -> {new} geometry bytes, {kept} retained features, {:.1}% smaller",
            100. * (1. - new as f64 / old as f64)
        );
    }
}
