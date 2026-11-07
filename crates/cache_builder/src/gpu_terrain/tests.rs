use super::*;
use crate::marching_squares::SrtmTile;
fn upload(worker: &Worker, data: &[f32]) -> wgpu::Buffer {
    worker.buffer(
        "test raster",
        bytemuck::cast_slice(data),
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    )
}
#[test]
fn invalid_grids_are_rejected_before_allocating() {
    let b = Bounds {
        min_lon: 10.,
        max_lon: 11.,
        min_lat: 20.,
        max_lat: 21.,
    };
    assert!(validate_grid(b, 3601).is_ok());
    for n in [0, 1, 6001, u32::MAX] {
        assert!(validate_grid(b, n).is_err());
    }
    assert!(
        validate_grid(
            Bounds {
                max_lat: f64::NAN,
                ..b
            },
            10
        )
        .is_err()
    );
    assert!(validate_grid(Bounds { max_lon: 190., ..b }, 10).is_err());
}
#[test]
fn source_axes_preserve_native_posts_and_seam_ownership() {
    let b = Bounds {
        min_lon: 36.99958333333333,
        max_lon: 37.00041666666667,
        min_lat: 29.99958333333333,
        max_lat: 30.00041666666667,
    };
    let west = axes(b, 3, 29, 36);
    let east = axes(b, 3, 30, 37);
    assert_eq!(&west[..3], &[3599., 3600., -1.]);
    assert_eq!(&east[..3], &[-1., 0., 1.]);
    assert_eq!(&west[3..], &[-1., 0., 1.]);
    assert_eq!(&east[3..], &[3599., 3600., -1.]);
}
#[test]
#[ignore = "requires a compute GPU"]
fn gpu_matches_cpu_cases_saddles_negative_levels_and_voids() {
    let worker = Worker::new().unwrap();
    let b = Bounds {
        min_lon: 0.,
        max_lon: 2.,
        min_lat: 0.,
        max_lat: 2.,
    };
    for code in 0..16 {
        let grid: Vec<f32> = [8, 4, 1, 2]
            .into_iter()
            .map(|bit| if code & bit != 0 { 2. } else { -2. })
            .collect();
        let gpu = worker.extract(&upload(&worker, &grid), 2, 1., 64).unwrap();
        for h in [-1., 0., 1.] {
            let mut cpu = crate::marching_squares::extract_segments(&grid, 2, h, b);
            if code == 5 || code == 10 {
                let interp = |a: f32, b: f32| (h - a) / (b - a);
                let n = (0.5 + interp(grid[0], grid[1]), 1.5);
                let e = (1.5, 1.5 - interp(grid[1], grid[3]));
                let s = (0.5 + interp(grid[2], grid[3]), 0.5);
                let w = (0.5, 1.5 - interp(grid[0], grid[2]));
                cpu = vec![[w, s], [n, e]];
            }
            let mut actual: Vec<_> = gpu
                .iter()
                .filter(|s| s[4] == h)
                .map(|s| [(s[0] + 0.5, 1.5 - s[1]), (s[2] + 0.5, 1.5 - s[3])])
                .collect();
            let key = |s: &[(f32, f32); 2]| {
                (
                    s[0].0.to_bits(),
                    s[0].1.to_bits(),
                    s[1].0.to_bits(),
                    s[1].1.to_bits(),
                )
            };
            cpu.sort_by_key(key);
            actual.sort_by_key(key);
            assert_eq!(actual, cpu, "case {code}, level {h}");
        }
    }
    let grid = [NODATA; 4];
    assert!(
        worker
            .extract(&upload(&worker, &grid), 2, 1., 64)
            .unwrap()
            .is_empty()
    );
    // Overflow must error, never produce a truncated but valid-looking tile.
    let grid: Vec<f32> = (0..64)
        .map(|i| if (i + i / 8) % 2 == 0 { 20. } else { -20. })
        .collect();
    assert!(
        worker
            .extract(&upload(&worker, &grid), 8, 1., 1)
            .unwrap_err()
            .contains("capacity exceeded")
    );
    assert!(worker.extract(&upload(&worker, &grid), 8, 0., 64).is_err());
}
#[test]
#[ignore = "requires a compute GPU"]
fn gpu_bilinear_interpolation_and_nodata() {
    let worker = Worker::new().unwrap();
    let source = upload(&worker, &[0., 10., 20., 30.]);
    let axes = upload(&worker, &[0., 0.5, 0., 0.5]);
    let params = worker.buffer(
        "test parameters",
        bytemuck::cast_slice(&[2u32, 2, 2, 0]),
        wgpu::BufferUsages::UNIFORM,
    );
    let grid = upload(&worker, &[NODATA; 4]);
    worker.dispatch(
        &worker.sample_pipeline,
        &[&params, &source, &axes, &grid],
        2,
    );
    let bytes = worker.read(&grid, 16).unwrap();
    assert_eq!(bytemuck::cast_slice::<u8, f32>(&bytes), &[0., 5., 10., 15.]);
    let grid = upload(&worker, &[NODATA; 4]);
    let source = upload(&worker, &[0., NODATA, 20., 30.]);
    worker.dispatch(
        &worker.sample_pipeline,
        &[&params, &source, &axes, &grid],
        2,
    );
    let bytes = worker.read(&grid, 16).unwrap();
    assert_eq!(
        bytemuck::cast_slice::<u8, f32>(&bytes),
        &[0., NODATA, 10., NODATA]
    );
}
#[test]
#[ignore = "requires ONEKEE_SRTM_ROOT, GDAL and compute GPU; writes only temporary files"]
fn gpu_native_raster_matches_gdal() {
    let root = PathBuf::from(std::env::var_os("ONEKEE_SRTM_ROOT").unwrap());
    let mut worker = Worker::new().unwrap();
    // Crosses both a latitude and longitude source boundary, including negative coordinates.
    let mut fixtures = Vec::new();
    for (lat, lon) in [(29i32, 37i32), (-25, -59)] {
        let step = 1. / 3600.;
        let n = 32u32;
        let b = Bounds {
            min_lon: f64::from(lon) - 16.5 * step,
            max_lon: f64::from(lon) + 15.5 * step,
            min_lat: f64::from(lat) - 16.5 * step,
            max_lat: f64::from(lat) + 15.5 * step,
        };
        fixtures.push((b, n));
    }
    for (half, lat, lon) in [(0.55, 126, 143), (3.6, 9, 27), (0.55, -98, -235)] {
        let core = CoreTile::srtm(half, lat, lon);
        fixtures.push((core.source, core.raster_size));
    }
    for (fixture, (b, n)) in fixtures.into_iter().enumerate() {
        let grid = worker.resample(&root, b, n).unwrap();
        let bytes = worker.read(&grid, u64::from(n) * u64::from(n) * 4).unwrap();
        let actual = bytemuck::cast_slice::<u8, f32>(&bytes);
        let tmp = std::env::temp_dir().join(format!(
            "1kee-gpu-raster-{}-{fixture}.tif",
            std::process::id()
        ));
        let mut cmd = std::process::Command::new("gdalwarp");
        cmd.env("GDAL_PAM_ENABLED", "NO")
            .args([
                "-q",
                "-overwrite",
                "-r",
                "bilinear",
                "-dstnodata",
                "-32768",
                "-te",
            ])
            .args([b.min_lon, b.min_lat, b.max_lon, b.max_lat].map(|v| format!("{v:.15}")))
            .args(["-ts", &n.to_string(), &n.to_string()]);
        for y in b.min_lat.floor() as i32..=b.max_lat.floor() as i32 {
            for x in b.min_lon.floor() as i32..=b.max_lon.floor() as i32 {
                let p = root.join(format!(
                    "{}{:02}{}{:03}.tif",
                    if y >= 0 { 'N' } else { 'S' },
                    y.unsigned_abs(),
                    if x >= 0 { 'E' } else { 'W' },
                    x.unsigned_abs()
                ));
                if p.exists() {
                    cmd.arg(p);
                }
            }
        }
        assert!(cmd.arg(&tmp).status().unwrap().success());
        let expected = SrtmTile::load(&tmp).unwrap();
        let mut valid = 0;
        for (&a, &e) in actual.iter().zip(&expected.samples) {
            if e.is_nan() {
                assert_eq!(a, NODATA);
            } else {
                valid += 1;
                assert!((a - e).abs() < 0.01, "fixture {fixture}: GPU {a}, GDAL {e}");
            }
        }
        assert!(valid > 0);
        std::fs::remove_file(tmp).unwrap();
    }
}

#[test]
#[ignore = "requires GDAL and compute GPU; synthetic ties/saddles/voids in temporary files"]
fn gpu_contours_match_gdal_degenerate_cells() {
    let worker = Worker::new().unwrap();
    let root = std::env::temp_dir().join(format!("1kee-gpu-cases-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let bounds = Bounds {
        min_lon: 0.5,
        max_lon: 1.5,
        min_lat: 0.5,
        max_lat: 1.5,
    };
    let mut cases = Vec::new();
    for code in 0..16 {
        cases.push([8, 4, 1, 2].map(|bit| if code & bit != 0 { 1.0 } else { -1.0 }));
        cases.push([8, 4, 1, 2].map(|bit| if code & bit != 0 { 0.0 } else { -1.0 }));
    }
    // Partial voids must terminate at half pixels, not discard entire cells.
    cases.extend([
        [NODATA, 1., -1., 1.],
        [1., NODATA, -1., NODATA],
        [NODATA; 4],
        [0., 0., 0., 0.],
    ]);
    for (i, values) in cases.into_iter().enumerate() {
        let tif = root.join(format!("{i}.tif"));
        let json = root.join(format!("{i}.json"));
        {
            use tiff::{
                encoder::{TiffEncoder, colortype::Gray32Float},
                tags::Tag,
            };
            let mut writer = TiffEncoder::new(std::fs::File::create(&tif).unwrap()).unwrap();
            let mut image = writer.new_image::<Gray32Float>(2, 2).unwrap();
            image
                .encoder()
                .write_tag(Tag::ModelPixelScaleTag, &[1f64, 1., 0.][..])
                .unwrap();
            image
                .encoder()
                .write_tag(Tag::ModelTiepointTag, &[0f64, 0., 0., 0., 2., 0.][..])
                .unwrap();
            image.write_data(&values).unwrap();
        }
        let status = std::process::Command::new("gdal_contour")
            .env("GDAL_PAM_ENABLED", "NO")
            .args([
                "-q",
                "-snodata",
                "-32768",
                "-fl",
                "0",
                "-a",
                "elevation",
                "-f",
                "GeoJSON",
            ])
            .arg(&tif)
            .arg(&json)
            .status()
            .unwrap();
        assert!(status.success());
        let data: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&json).unwrap()).unwrap();
        let gdal: Vec<Vec<(f64, f64)>> = data["features"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| {
                f["geometry"]["coordinates"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|p| (p[0].as_f64().unwrap(), p[1].as_f64().unwrap()))
                    .collect()
            })
            .collect();
        let raw = worker
            .extract(&upload(&worker, &values), 2, 1., 128)
            .unwrap();
        let gpu: Vec<Vec<(f64, f64)>> = raw
            .iter()
            .filter(|s| s[4] == 0.)
            .map(|s| {
                vec![
                    (0.5 + f64::from(s[0]), 1.5 - f64::from(s[1])),
                    (0.5 + f64::from(s[2]), 1.5 - f64::from(s[3])),
                ]
            })
            .collect();
        let normalize = |lines: Vec<Vec<(f64, f64)>>| {
            let mut result = Vec::new();
            for line in lines {
                for clipped in tile_archive::contour_grid::clip_line(&line, bounds) {
                    for pair in clipped.windows(2) {
                        let mut edge = pair
                            .iter()
                            .map(|p| {
                                (
                                    (p.0 * 100000.).round() as i64,
                                    (p.1 * 100000.).round() as i64,
                                )
                            })
                            .collect::<Vec<_>>();
                        edge.sort();
                        if edge[0] != edge[1] {
                            result.push(edge);
                        }
                    }
                }
            }
            result.sort();
            result
        };
        assert_eq!(normalize(gpu), normalize(gdal), "fixture {i}: {values:?}");
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn windowed_strips_preserve_edges_nodata_and_skip_unneeded_rows() {
    use tiff::encoder::{TiffEncoder, colortype::GrayI16};
    let path = std::env::temp_dir().join(format!(
        "1kee-source-window-{}-{}.tif",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let value = |x: u32, y: u32| {
        if x == 3600 && y == 3600 {
            i16::MIN
        } else {
            ((x % 2000 + y % 2000) as i32 - 2000) as i16
        }
    };
    let samples: Vec<i16> = (0..3601u32)
        .flat_map(|y| (0..3601u32).map(move |x| value(x, y)))
        .collect();
    {
        let mut encoder = TiffEncoder::new(std::fs::File::create(&path).unwrap()).unwrap();
        let mut image = encoder.new_image::<GrayI16>(3601, 3601).unwrap();
        image.rows_per_strip(16).unwrap();
        image.write_data(&samples).unwrap();
    }
    let size = path.metadata().unwrap().len();
    for requested in [
        Window {
            x: 122,
            y: 127,
            width: 3,
            height: 3,
        },
        Window {
            x: 3599,
            y: 3598,
            width: 2,
            height: 3,
        },
    ] {
        let decoded = window::load(&path, requested).unwrap();
        assert!(decoded.window.contains(requested));
        assert_eq!(decoded.chunks, 2);
        assert!(
            decoded.bytes_read < size / 4,
            "read {} of {size} bytes",
            decoded.bytes_read
        );
        for (i, &sample) in decoded.samples.iter().enumerate() {
            let x = decoded.window.x + i as u32 % decoded.window.width;
            let y = decoded.window.y + i as u32 / decoded.window.width;
            assert_eq!(sample, f32::from(value(x, y)));
        }
    }
    assert!(
        window::load(
            &path,
            Window {
                x: 3600,
                y: 0,
                width: 2,
                height: 1
            }
        )
        .is_err()
    );
    std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(size / 2)
        .unwrap();
    assert!(
        window::load(
            &path,
            Window {
                x: 0,
                y: 0,
                width: 2,
                height: 2
            }
        )
        .is_err()
    );
    std::fs::remove_file(path).unwrap();
}

#[test]
fn source_windows_include_bilinear_neighbors_without_crossing_the_source() {
    assert_eq!(
        Window::from_axes(&[122.5, 123., 3599.5, 3600.], 2),
        Some(Window {
            x: 122,
            y: 3599,
            width: 3,
            height: 2
        })
    );
    assert_eq!(Window::from_axes(&[-1., -1., 20., 21.], 2), None);
    assert_eq!(Window::from_axes(&[1.], 2), None);
}
