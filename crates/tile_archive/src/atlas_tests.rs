use super::*;
fn surface(key: Key, f: impl Fn(f64, f64) -> f32) -> Vec<f32> {
    let b = key.source_bounds();
    let n = key.tier.side();
    let step = key.tier.spacing() / 3600.;
    (0..n)
        .flat_map(|y| {
            let f = &f;
            (0..n).map(move |x| {
                f(
                    b[0] + (x as f64 + 0.5) * step,
                    b[3] - (y as f64 + 0.5) * step,
                )
            })
        })
        .collect()
}
#[test]
fn geographic_plan_handles_world_edges_and_dateline_without_duplicates() {
    assert_eq!(
        plan([-180., -90., 180., 90.], &[Tier::Overview])
            .unwrap()
            .len(),
        648
    );
    let keys = plan([179.5, -0.5, 180.5, 0.5], &[Tier::Terrain, Tier::Terrain]).unwrap();
    assert_eq!(keys.len(), 4);
    assert!(keys.iter().all(|k| k.valid()));
    assert!(plan([0., 0., 0., 1.], &[Tier::Terrain]).is_err());
    assert!(plan([0., -91., 1., 1.], &[Tier::Land]).is_err());
}
#[test]
fn adjacent_halos_and_dateline_resample_continuously() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join(FILE_NAME);
    let writer = Writer::open(&path).unwrap();
    for key in plan([-1., -1., 1., 1.], &[Tier::Terrain]).unwrap() {
        writer
            .put(
                key,
                "fixture",
                &surface(key, |lon, lat| (lon + 2. * lat) as f32),
            )
            .unwrap();
    }
    let mut reader = Reader::open(&path).unwrap();
    let b = [-0.8, -0.7, 0.9, 0.6];
    let values = reader.sample(b, [50, 40], Tier::Terrain).unwrap().unwrap();
    for y in 0..40 {
        for x in 0..50 {
            let expected = b[0]
                + (x as f64 + 0.5) * (b[2] - b[0]) / 50.
                + 2. * (b[3] - (y as f64 + 0.5) * (b[3] - b[1]) / 40.);
            assert!((values[y * 50 + x] as f64 - expected).abs() < 1e-5);
        }
    }
    for key in plan([179., -1., 181., 1.], &[Tier::Terrain]).unwrap() {
        writer
            .put(
                key,
                "fixture",
                &surface(key, |lon, lat| (100. * lon.to_radians().cos() + lat) as f32),
            )
            .unwrap();
    }
    let values = reader
        .sample([179.5, -0.5, 180.5, 0.5], [100, 100], Tier::Terrain)
        .unwrap()
        .unwrap();
    assert!(values.iter().all(|v| v.is_finite()));
    assert!((values[49] - values[50]).abs() < 0.01);
    assert!(
        reader
            .sample([10., 10., 11., 11.], [16, 16], Tier::Terrain)
            .unwrap()
            .is_none()
    );
}
#[test]
fn resume_checks_source_and_payload_and_rejects_foreign_databases() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join(FILE_NAME);
    let key = Key::at(Tier::Terrain, 0.5, 0.5);
    let values = surface(key, |_, _| 123.);
    {
        let writer = Writer::open(&path).unwrap();
        writer.put(key, "source-a", &values).unwrap();
        assert!(writer.reusable(key, "source-a").unwrap());
        assert!(!writer.reusable(key, "source-b").unwrap());
        assert!(Writer::open(&path).is_err());
        assert!(writer.put(key, "source-a", &[12.]).is_err());
    }
    {
        let writer = Writer::open(&path).unwrap();
        writer
            .connection
            .execute("UPDATE heights SET crc=0", [])
            .unwrap();
        assert!(writer.reusable(key, "source-a").is_err());
        writer.put(key, "source-a", &values).unwrap();
        writer.finish().unwrap();
    }
    assert!(
        Reader::open(&path)
            .unwrap()
            .sample([0., 0., 1., 1.], [16, 16], Tier::Terrain)
            .unwrap()
            .unwrap()
            .iter()
            .all(|v| *v == 123.)
    );
    let foreign = tmp.path().join("foreign.sqlite");
    Connection::open(&foreign)
        .unwrap()
        .execute_batch("CREATE TABLE keep(value TEXT); INSERT INTO keep VALUES('unchanged');")
        .unwrap();
    let before = std::fs::read(&foreign).unwrap();
    assert!(Writer::open(&foreign).is_err());
    assert_eq!(std::fs::read(foreign).unwrap(), before);
}

#[test]
fn wide_views_read_each_tile_once_despite_a_smaller_lru() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(FILE_NAME);
    let writer = Writer::open(&path).unwrap();
    for key in plan([-35., 0., 35., 1.], &[Tier::Terrain]).unwrap() {
        writer
            .put(key, "wide", &vec![100.; Tier::Terrain.side().pow(2)])
            .unwrap();
    }
    let mut reader = Reader::open(&path).unwrap();
    let values = reader
        .sample([-35., 0.1, 35., 0.9], [700, 32], Tier::Terrain)
        .unwrap()
        .unwrap();
    assert!(values.iter().all(|v| (*v - 100.).abs() < 1e-3));
    assert_eq!(
        reader.tick, 70,
        "decode once per tile, not once per scanline"
    );
    assert_eq!(reader.tiles.len(), 64);
}
