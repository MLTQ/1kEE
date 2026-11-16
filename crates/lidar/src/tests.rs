use super::*;
use crate::render::{SIZE, read_mean, render};

fn bounds() -> [f64; 4] {
    let span = 180. / 32768.;
    let w = -180. + 18640. * span;
    let n = 90. - 9549. * span;
    [w, n - span, w + span, n]
}
fn scan(
    root: &Path,
    name: &str,
    columns: &[u32],
    heights: &[f64],
    classes: &[u8],
    withheld: &[bool],
    date: &str,
) -> Asset {
    let [w, s, e, n] = bounds();
    let step = (e - w) / 512.;
    let mut builder = las::Builder::from(las::Version::new(1, 4));
    builder.transforms = las::Vector {
        x: las::Transform {
            scale: 1e-9,
            offset: w,
        },
        y: las::Transform {
            scale: 1e-9,
            offset: s,
        },
        z: las::Transform {
            scale: 0.001,
            offset: 0.,
        },
    };
    let mut header = builder.into_header().unwrap();
    header
        .set_wkt_crs(
            SpatialRef::from_epsg(4326)
                .unwrap()
                .to_wkt()
                .unwrap()
                .into_bytes(),
        )
        .unwrap();
    let source = root.join(name);
    let mut writer = las::Writer::from_path(&source, header).unwrap();
    for i in 0..heights.len() {
        writer
            .write_point(las::Point {
                x: w + (columns[i] as f64 + 0.25) * step,
                y: n - 100.25 * step,
                z: heights[i],
                classification: las::point::Classification::new(classes[i]).unwrap(),
                is_withheld: withheld[i],
                ..Default::default()
            })
            .unwrap();
    }
    writer.close().unwrap();
    Asset {
        url: format!("https://example.org/{name}"),
        survey: name.into(),
        source,
        date: date.into(),
    }
}
fn setup(root: &Path) -> Connection {
    fs::create_dir_all(root.join("fused/v1")).unwrap();
    let db = Connection::open(root.join("fused/v1/index.sqlite")).unwrap();
    db.execute_batch(SCHEMA).unwrap();
    let catalog = Connection::open(root.join("catalog.sqlite")).unwrap();
    catalog
        .execute_batch("CREATE TABLE surveys (id TEXT PRIMARY KEY,name TEXT,date TEXT,page TEXT);")
        .unwrap();
    db
}
fn register(root: &Path, asset: &Asset) {
    Connection::open(root.join("catalog.sqlite"))
        .unwrap()
        .execute(
            "INSERT INTO surveys VALUES (?1,?1,?2,'https://example.org')",
            params![asset.survey, asset.date],
        )
        .unwrap();
}

#[test]
fn las_and_laz_preserve_independently_reduced_statistics_and_noise_masks() {
    for ext in ["las", "laz"] {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let db = setup(root);
        let asset = scan(
            root,
            &format!("cloud.{ext}"),
            &[100, 100, 300, 300, 300, 300],
            &[10., 20., 30., 900., 800., 700.],
            &[2, 2, 2, 7, 2, 18],
            &[false, false, false, false, true, false],
            "2026",
        );
        assert_eq!(prepare(root, &db, &asset, 15).unwrap(), "built");
        let (path, points, accepted, cells): (String, u64, u64, u64) = db
            .query_row(
                "SELECT path,points,valid_points,valid_cells FROM rasters",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!((points, accepted, cells), (6, 3, 2));
        assert_eq!(verify(root).unwrap()["verified"], 1);
        let values = read_mean(&root.join(path), bounds()).unwrap();
        assert_eq!(values[100 * SIZE + 100], 15.);
        assert_eq!(values[100 * SIZE + 300], 30.);
        assert_eq!(values.iter().filter(|v| v.is_finite()).count(), 2);
        assert!(asset.source.is_file());
    }
}

#[test]
fn newest_measured_pixels_win_and_older_acquisition_fills_only_gaps_without_raw_sources() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let db = setup(root);
    let old = scan(
        root,
        "old.las",
        &[100, 300],
        &[10., 10.],
        &[2, 2],
        &[false, false],
        "2020",
    );
    let new = scan(root, "new.las", &[100], &[20.], &[2], &[false], "2026");
    for asset in [&old, &new] {
        register(root, asset);
        prepare(root, &db, asset, 15).unwrap();
        fs::remove_file(&asset.source).unwrap();
    }
    let image = render(&root.join("catalog.sqlite"), bounds(), "", None)
        .unwrap()
        .unwrap();
    assert_eq!(
        image
            .credits
            .iter()
            .map(|c| c[1].as_str())
            .collect::<Vec<_>>(),
        vec!["new.las", "old.las"]
    );
    let expected = crate::render::shade(&vec![20.; SIZE * SIZE], bounds());
    assert_eq!(
        &image.rgba[(100 * SIZE + 100) * 4..(100 * SIZE + 100) * 4 + 4],
        &expected[(100 * SIZE + 100) * 4..(100 * SIZE + 100) * 4 + 4]
    );
    assert_eq!(image.rgba[(100 * SIZE + 300) * 4 + 3], 255);
    assert_eq!(image.rgba[(100 * SIZE + 200) * 4 + 3], 0);
    let history = render(&root.join("catalog.sqlite"), bounds(), "old.las", None)
        .unwrap()
        .unwrap();
    assert_eq!(history.credits[0][1], "old.las");
    assert_eq!(history.credits.len(), 1);
}

#[test]
fn missing_new_raster_does_not_hide_other_available_measurements() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let db = setup(root);
    let old = scan(
        root,
        "available.las",
        &[100],
        &[10.],
        &[2],
        &[false],
        "2020",
    );
    let new = scan(root, "missing.las", &[100], &[20.], &[2], &[false], "2026");
    for asset in [&old, &new] {
        register(root, asset);
        prepare(root, &db, asset, 15).unwrap();
    }
    let missing: String = db
        .query_row(
            "SELECT path FROM rasters WHERE survey=?1",
            [&new.survey],
            |r| r.get(0),
        )
        .unwrap();
    fs::remove_file(root.join(missing)).unwrap();
    let image = crate::render::render_prepared(&root.join("catalog.sqlite"), bounds(), None)
        .unwrap()
        .unwrap();
    assert_eq!(image.credits[0][1], "available.las");
    assert_eq!(image.rgba[(100 * SIZE + 100) * 4 + 3], 255);
}

#[test]
fn resume_repairs_corruption_and_date_updates_invalidate_the_receipt() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let db = setup(root);
    let mut asset = scan(
        root,
        "a.las",
        &[100, 300],
        &[10., 20.],
        &[2, 2],
        &[false, false],
        "2020",
    );
    prepare(root, &db, &asset, 15).unwrap();
    assert_eq!(prepare(root, &db, &asset, 15).unwrap(), "reused");
    let (id, path): (i64, String) = db
        .query_row("SELECT id,path FROM rasters", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    fs::write(root.join(path), "broken").unwrap();
    assert_eq!(
        verify(root).unwrap()["failures"].as_array().unwrap().len(),
        1
    );
    assert_eq!(prepare(root, &db, &asset, 15).unwrap(), "built");
    assert_eq!(verify(root).unwrap()["verified"], 1);
    asset.date = "2026".into();
    assert_eq!(prepare(root, &db, &asset, 15).unwrap(), "built");
    let new_id: i64 = db
        .query_row("SELECT id FROM rasters", [], |r| r.get(0))
        .unwrap();
    assert!(new_id > id);
}

#[test]
fn truncated_cloud_never_publishes_a_receipt_or_removes_raw() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let db = setup(root);
    let asset = scan(
        root,
        "a.las",
        &[100, 300],
        &[10., 20.],
        &[2, 2],
        &[false, false],
        "2026",
    );
    let size = asset.source.metadata().unwrap().len();
    File::options()
        .write(true)
        .open(&asset.source)
        .unwrap()
        .set_len(size - 5)
        .unwrap();
    assert!(prepare(root, &db, &asset, 15).is_err());
    assert!(asset.source.is_file());
    assert_eq!(
        db.query_row("SELECT count(*) FROM rasters", [], |r| r.get::<_, u32>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        fs::read_dir(root.join("fused/v1/rasters")).unwrap().count(),
        0
    );
}

#[test]
fn pending_scan_delegates_and_cancelled_view_stops_native_work() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let db = setup(root);
    let asset = scan(root, "a.las", &[100], &[10.], &[2], &[false], "2026");
    register(root, &asset);
    prepare(root, &db, &asset, 15).unwrap();
    assert!(
        render(
            &root.join("catalog.sqlite"),
            bounds(),
            "",
            Some(&root.join("gone"))
        )
        .unwrap_err()
        .contains("Camera moved")
    );
    let bulk = Connection::open(root.join("bulk.sqlite")).unwrap();
    bulk.execute_batch(
        "CREATE TABLE files (url TEXT,survey TEXT,west REAL,south REAL,east REAL,north REAL);",
    )
    .unwrap();
    let [w, s, e, n] = bounds();
    bulk.execute(
        "INSERT INTO files VALUES ('pending','new',?1,?2,?3,?4)",
        params![w, s, e, n],
    )
    .unwrap();
    assert!(
        render(&root.join("catalog.sqlite"), bounds(), "", None)
            .unwrap()
            .is_none()
    );
    // The sidebar's prepared-only layer still shows completed pixels while
    // neighboring raw scans wait in the conversion queue.
    let prepared = crate::render::render_prepared(&root.join("catalog.sqlite"), bounds(), None)
        .unwrap()
        .unwrap();
    assert!(prepared.rgba.as_chunks::<4>().0.iter().any(|p| p[3] > 0));
}

#[test]
fn planar_and_pixel_interleaved_tiff_chunks_have_identical_means_including_edge_tiles() {
    let tmp = tempfile::tempdir().unwrap();
    let [w, s, e, n] = bounds();
    let _ = s;
    for planar in [false, true] {
        let path = tmp.path().join(format!("{planar}.tif"));
        let mut ds = Dataset::create_layout(&path, 513, 513, planar).unwrap();
        ds.set_spatial_ref(&SpatialRef::from_epsg(4326).unwrap())
            .unwrap();
        let res = (e - w) / 512.;
        ds.set_geo_transform(&[w, res, 0., n, 0., -res]).unwrap();
        for band in 1..=4 {
            let mut b = ds.rasterband(band).unwrap();
            b.set_no_data_value(f64::NAN).unwrap();
            let mut data = (0..513 * 513)
                .map(|i| if i % 513 == 250 { f32::NAN } else { i as f32 })
                .collect::<Vec<_>>();
            b.write(&mut data).unwrap();
        }
        ds.flush_cache().unwrap();
        drop(ds);
        let values = read_mean(&path, [w, n - 513. * res, w + 513. * res, n]).unwrap();
        assert_eq!(values[0], 0.);
        assert_eq!(values[SIZE * SIZE - 1], (512 * 513 + 512) as f32);
        assert!(values.iter().any(|v| v.is_nan()));
    }
}
