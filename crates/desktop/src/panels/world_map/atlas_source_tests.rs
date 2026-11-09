use super::*;

#[test]
fn fine_land_keeps_ocean_depth_and_coarse_nodata_fallback() {
    let mut base = [-4000., -20., 300., 600., 20., f32::NAN];
    merge_land(&mut base, &[0., 150., -999999., 712., f32::NAN, 100.]);
    assert_eq!(base, [-4000., 150., 300., 712., 20., 100.]);
}

#[test]
fn source_selection_uses_only_intersecting_local_tiles_and_wraps() {
    let dir = tempfile::tempdir().unwrap();
    for name in [
        "N45E005.tif",
        "N45E006.tif",
        "N45E179.tif",
        "N45W180.tif",
        "N46E006.tif",
    ] {
        std::fs::write(dir.path().join(name), []).unwrap();
    }
    let paths = srtm_files(dir.path(), [5.2, 45.1, 6.2, 45.9]);
    assert_eq!(paths.len(), 2);
    let paths = srtm_files(dir.path(), [179.8, 45.1, 180.2, 45.9]);
    assert_eq!(paths.len(), 2);
    assert!(paths.iter().any(|p| p.ends_with("N45W180.tif")));
}

#[test]
#[ignore = "requires installed GDAL and original GEBCO terrain files"]
fn native_europe_relief_retains_measured_ridges() {
    let root = PathBuf::from(std::env::var("ONEKEE_ATLAS_ROOT").expect("existing terrain root"));
    let region = Region([-6 * 3600, 40 * 3600, 20 * 3600, 55 * 3600]);
    let detail = load(Some(&root), region, 8192).expect("original GEBCO window");
    assert_eq!([detail.grid.width, detail.grid.height], [6240, 3600]);
    assert!(detail.source.starts_with("GEBCO"));
    assert!(detail.grid.metres.iter().any(|&v| v > 3500.));
    assert!(detail.grid.metres.iter().any(|&v| v < -2000.));
    let image = detail.grid.image_for(region.bounds()).unwrap();
    if let Ok(path) = std::env::var("ONEKEE_ATLAS_NATIVE_PREVIEW") {
        let bytes: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_array()).collect();
        image::save_buffer(
            path,
            &bytes,
            image.size[0] as u32,
            image.size[1] as u32,
            image::ColorType::Rgba8,
        )
        .unwrap();
    }
}

#[test]
#[ignore = "requires installed GDAL and original GEBCO/SRTM terrain files"]
fn native_alpine_zoom_uses_srtm_posts() {
    let root = PathBuf::from(std::env::var("ONEKEE_ATLAS_ROOT").expect("existing terrain root"));
    let region = Region([7 * 3600, 46 * 3600, 8 * 3600, 47 * 3600]);
    let detail = load(Some(&root), region, 8192).expect("original SRTM window");
    assert_eq!([detail.grid.width, detail.grid.height], [3600, 3600]);
    assert!(detail.source.starts_with("SRTM"));
    assert!(detail.grid.metres.iter().any(|&v| v > 4000.));
}

#[test]
#[ignore = "requires installed GDAL; uses private synthetic georeferenced fixture"]
fn native_warp_preserves_dateline_pixel_centres() {
    let dir = tempfile::tempdir().unwrap();
    let mut bytes = Vec::new();
    for _ in 0..180 {
        for x in 0..360 {
            bytes.extend_from_slice(&(x as f32 + 100.0).to_le_bytes());
        }
    }
    std::fs::write(dir.path().join("heights.bin"), bytes).unwrap();
    let path = dir.path().join("source.vrt");
    std::fs::write(&path,r#"<VRTDataset rasterXSize="360" rasterYSize="180"><SRS>EPSG:4326</SRS><GeoTransform>-180,1,0,90,0,-1</GeoTransform><VRTRasterBand dataType="Float32" band="1" subClass="VRTRawRasterBand"><SourceFilename relativeToVRT="1">heights.bin</SourceFilename><ImageOffset>0</ImageOffset><PixelOffset>4</PixelOffset><LineOffset>1440</LineOffset><ByteOrder>LSB</ByteOrder></VRTRasterBand></VRTDataset>"#).unwrap();
    let values = warp(
        &[path],
        [179., -1., 181., 1.],
        [8, 8],
        dir.path(),
        "wrapped",
    )
    .unwrap();
    assert!(values.iter().all(|&v| v.is_finite() && v > 0.));
    for row in values.chunks_exact(8) {
        assert!(row[0] > 450. && row[7] < 110.);
    }
}
