use super::*;
#[test]
fn atlas_cli_parses_tiers_and_dateline_and_rejects_bad_input() {
    let c = parse(
        [
            "--data-root",
            "/tmp/data",
            "--out",
            "/tmp/atlas.sqlite",
            "--tiers",
            "terrain,land",
            "--min-lon",
            "179",
            "--max-lon",
            "181",
        ]
        .map(String::from)
        .into_iter(),
    )
    .unwrap();
    assert_eq!(c.bounds, [179., -90., 181., 90.]);
    assert_eq!(c.tiers, vec![Tier::Terrain, Tier::Land]);
    assert!(
        parse(
            ["--data-root", "x", "--out", "y", "--tiers", "mystery"]
                .map(String::from)
                .into_iter()
        )
        .is_err()
    );
    assert!(
        parse(
            ["--data-root", "x", "--out", "y", "--min-lat", "91"]
                .map(String::from)
                .into_iter()
        )
        .is_err()
    );
}
#[test]
#[ignore = "requires installed GDAL; synthetic VRT and cache live only in /tmp"]
fn gdal_atlas_build_resumes_repairs_and_publishes_measured_tiles() {
    let scratch = Scratch::new().unwrap();
    let dir = &scratch.0;
    let values = vec![100f32; 360 * 180];
    fs::write(
        dir.join("heights.bin"),
        values
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect::<Vec<_>>(),
    )
    .unwrap();
    fs::write(dir.join("source.vrt"),r#"<VRTDataset rasterXSize="360" rasterYSize="180"><SRS>EPSG:4326</SRS><GeoTransform>-180,1,0,90,0,-1</GeoTransform><VRTRasterBand dataType="Float32" band="1" subClass="VRTRawRasterBand"><SourceFilename relativeToVRT="1">heights.bin</SourceFilename><ImageOffset>0</ImageOffset><PixelOffset>4</PixelOffset><LineOffset>1440</LineOffset><ByteOrder>LSB</ByteOrder></VRTRasterBand></VRTDataset>"#).unwrap();
    let c = Command {
        data_root: dir.join("source.vrt"),
        srtm_root: None,
        cache: dir.join("atlas.sqlite"),
        gdal_bin_dir: PathBuf::new(),
        bounds: [179.5, -0.25, 180.5, 0.25],
        tiers: vec![Tier::Terrain],
    };
    let mut latest = Progress::default();
    // Publish a partial selection, then resume a larger request against it.
    let mut partial = c.clone();
    partial.bounds = [179.5, 0.1, 179.9, 0.2];
    run(partial, &mut |p| latest = p).unwrap();
    assert_eq!(latest.built, 1);
    run(c.clone(), &mut |p| latest = p).unwrap();
    assert_eq!(latest.built, 3);
    assert_eq!(latest.reused, 1);
    assert_eq!(latest.done, 4);
    run(c.clone(), &mut |p| latest = p).unwrap();
    assert_eq!(latest.built, 0);
    assert_eq!(latest.reused, 4);
    rusqlite::Connection::open(&c.cache)
        .unwrap()
        .execute("UPDATE heights SET crc=0 WHERE x=0 AND y=90", [])
        .unwrap();
    run(c.clone(), &mut |p| latest = p).unwrap();
    assert_eq!(latest.built, 1);
    assert_eq!(latest.reused, 3);
    let values = tile_archive::atlas::Reader::open(&c.cache)
        .unwrap()
        .sample(c.bounds, [128, 64], Tier::Terrain)
        .unwrap()
        .unwrap();
    assert!(values.iter().all(|v| (*v - 100.).abs() < 1e-3));
    let mut pole = c.clone();
    pole.tiers = vec![Tier::Overview];
    pole.bounds = [-180., -90., -179.9, -89.9];
    run(pole.clone(), &mut |p| latest = p).unwrap();
    assert_eq!(latest.built, 1);
    let pole_values = tile_archive::atlas::Reader::open(&pole.cache)
        .unwrap()
        .sample(pole.bounds, [16, 16], Tier::Overview)
        .unwrap()
        .unwrap();
    assert!(pole_values.iter().all(|v| (*v - 100.).abs() < 1e-3));
    // A changed source identity must rebuild, even though every checksum passes.
    let original = fs::read_to_string(&c.data_root).unwrap();
    fs::write(
        &c.data_root,
        format!("{original}\n<!-- new source revision -->"),
    )
    .unwrap();
    run(c.clone(), &mut |p| latest = p).unwrap();
    assert_eq!(latest.built, 4);
    assert_eq!(latest.reused, 0);

    // Fail a later tile by removing only the private GDAL launcher. Already
    // published tiles remain reusable after the tool becomes available again.
    #[cfg(unix)]
    {
        let bin = dir.join("bin");
        fs::create_dir(&bin).unwrap();
        let launcher = bin.join("gdalwarp");
        fs::write(&launcher, "#!/bin/sh\nexec gdalwarp \"$@\"\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&launcher, fs::Permissions::from_mode(0o700)).unwrap();
        let mut interrupted = c.clone();
        interrupted.cache = dir.join("interrupted.sqlite");
        interrupted.gdal_bin_dir = bin;
        assert!(
            run(interrupted.clone(), &mut |p| {
                if p.done == 1 && launcher.exists() {
                    fs::remove_file(&launcher).unwrap();
                }
                latest = p;
            })
            .is_err()
        );
        assert_eq!(latest.state, "Failed");
        assert_eq!(latest.done, 1);
        interrupted.gdal_bin_dir = PathBuf::new();
        run(interrupted, &mut |p| latest = p).unwrap();
        assert_eq!(latest.reused, 1);
        assert_eq!(latest.built, 3);
    }
}

#[test]
#[ignore = "requires installed GDAL; all rasters and output are private fixtures"]
fn gdal_atlas_land_detail_preserves_ocean_depth_and_srtm_heights() {
    let scratch = Scratch::new().unwrap();
    let dir = &scratch.0;
    fs::write(
        dir.join("ocean.bin"),
        vec![-800f32; 360 * 180]
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect::<Vec<_>>(),
    )
    .unwrap();
    fs::write(dir.join("ocean.vrt"), r#"<VRTDataset rasterXSize="360" rasterYSize="180"><SRS>EPSG:4326</SRS><GeoTransform>-180,1,0,90,0,-1</GeoTransform><VRTRasterBand dataType="Float32" band="1" subClass="VRTRawRasterBand"><SourceFilename relativeToVRT="1">ocean.bin</SourceFilename><ImageOffset>0</ImageOffset><PixelOffset>4</PixelOffset><LineOffset>1440</LineOffset><ByteOrder>LSB</ByteOrder></VRTRasterBand></VRTDataset>"#).unwrap();
    let land: Vec<f32> = (0..100 * 100)
        .map(|i| if i % 100 < 50 { 0. } else { 1200. })
        .collect();
    fs::write(
        dir.join("land.bin"),
        land.into_iter()
            .flat_map(f32::to_le_bytes)
            .collect::<Vec<_>>(),
    )
    .unwrap();
    // GDAL identifies this small georeferenced fixture by its VRT header.
    fs::write(dir.join("N00E000.tif"), r#"<VRTDataset rasterXSize="100" rasterYSize="100"><SRS>EPSG:4326</SRS><GeoTransform>0,0.01,0,1,0,-0.01</GeoTransform><VRTRasterBand dataType="Float32" band="1" subClass="VRTRawRasterBand"><SourceFilename relativeToVRT="1">land.bin</SourceFilename><ImageOffset>0</ImageOffset><PixelOffset>4</PixelOffset><LineOffset>400</LineOffset><ByteOrder>LSB</ByteOrder></VRTRasterBand></VRTDataset>"#).unwrap();
    let c = Command {
        data_root: dir.join("ocean.vrt"),
        srtm_root: Some(dir.clone()),
        cache: dir.join("land.sqlite"),
        gdal_bin_dir: PathBuf::new(),
        bounds: [0.1, 0.1, 1.1, 0.2],
        tiers: vec![Tier::Land],
    };
    let mut latest = Progress::default();
    run(c.clone(), &mut |p| latest = p).unwrap();
    assert_eq!(latest.built, 4);
    assert_eq!(latest.unavailable, 1);
    let values = tile_archive::atlas::Reader::open(&c.cache)
        .unwrap()
        .sample([0.1, 0.1, 0.9, 0.2], [128, 16], Tier::Land)
        .unwrap()
        .unwrap();
    for row in values.as_chunks::<128>().0 {
        assert!(
            (row[0] + 800.).abs() < 0.001,
            "SRTM sea zeros must retain measured depths"
        );
        assert!(
            (row[127] - 1200.).abs() < 0.001,
            "SRTM land must retain measured heights"
        );
    }
}
