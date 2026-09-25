use super::*;

fn request(lat: f64, lon: f64) -> Request {
    Request {
        bounds: Bounds {
            min_lon: lon - 0.001,
            max_lon: lon + 0.001,
            min_lat: lat - 0.001,
            max_lat: lat + 0.001,
        },
        width: 64,
        height: 64,
    }
}

#[test]
fn country_screens_are_fast_candidates_not_global_requests() {
    for (lat, lon) in [
        (51.50, -0.128),
        (52.37, 4.89),
        (46.95, 7.44),
        (48.85, 2.35),
        (-41.29, 174.775),
        (35.68, 139.77),
    ] {
        assert!(possible_at(GeoPoint { lat, lon }));
    }
    for (lat, lon) in [(40.7, -74.0), (-33.86, 151.21), (0.0, 0.0), (f32::NAN, 0.0)] {
        assert!(!possible_at(GeoPoint { lat, lon }));
    }
}

#[test]
fn queries_use_numeric_terrain_and_longitude_first_bounds() {
    let request = request(48.85, 2.35);
    for provider in [Provider::England, Provider::Netherlands, Provider::France] {
        let url = reqwest::Url::parse(&services::request_url(provider, request).unwrap()).unwrap();
        let pairs = url
            .query_pairs()
            .map(|(k, v)| (k.to_lowercase(), v.into_owned()))
            .collect::<std::collections::HashMap<_, _>>();
        assert_eq!(pairs["width"], "64");
        let b: Vec<f64> = pairs["bbox"]
            .split(',')
            .map(|s| s.parse().unwrap())
            .collect();
        assert_eq!(
            b,
            [
                request.bounds.min_lon,
                request.bounds.min_lat,
                request.bounds.max_lon,
                request.bounds.max_lat
            ]
        );
        assert!(!url.as_str().contains("SHADOW"));
        if provider == Provider::France {
            assert_eq!(pairs["crs"], "CRS:84");
        }
    }
}

#[test]
fn untrusted_catalog_urls_and_oversized_requests_are_rejected() {
    for url in [
        "file:///etc/passwd",
        "https://localhost/a.tif",
        "http://data.geo.admin.ch/a.tif",
        "https://data.geo.admin.ch.evil.example/a.tif",
        "https://user@data.geo.admin.ch/a.tif",
    ] {
        assert!(http::validate_url(url).is_err());
    }
    let mut request = request(48.85, 2.35);
    assert!(request.valid());
    request.width = u32::MAX;
    assert!(!request.valid());
    request.width = 64;
    request.bounds.max_lat = 50.0;
    assert!(!request.valid());
    request.bounds.max_lat = f64::NAN;
    assert!(!request.valid());
}

#[test]
fn normalized_grid_keeps_zero_negative_heights_and_rejects_empty_coverage() {
    let root = std::env::temp_dir().join(format!("1kee-provider-unit-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let scratch = Scratch::new(&root.join("test.tif")).unwrap();
    let request = request(48.85, 2.35);
    let samples: Vec<_> = (0..64 * 64)
        .map(|i| {
            if i < 200 {
                NODATA
            } else {
                i as f32 / 100.0 - 10.0
            }
        })
        .collect();
    let vrt = raster::float_grid(&samples, request, &scratch).unwrap();
    let output = raster::warp(
        &[vrt.to_string_lossy().into_owned()],
        request,
        &scratch,
        false,
    )
    .unwrap();
    assert!(output.exists());
    let checked = std::fs::read(scratch.path("check.bil")).unwrap();
    let decoded: Vec<_> = checked
        .chunks_exact(4)
        .map(|p| f32::from_le_bytes(p.try_into().unwrap()))
        .collect();
    for (index, (actual, expected)) in decoded.iter().zip(&samples).enumerate() {
        assert!(
            (actual - expected).abs() < 1e-6,
            "sample {index}: {actual} vs {expected}"
        );
    }
    let vrt = raster::float_grid(&vec![NODATA; 64 * 64], request, &scratch).unwrap();
    assert!(matches!(
        raster::warp(
            &[vrt.to_string_lossy().into_owned()],
            request,
            &scratch,
            false
        ),
        Err(Error::NoCoverage)
    ));
    let dir = scratch.0.clone();
    drop(scratch);
    assert!(!dir.exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore = "small official-service downloads; run explicitly with network access"]
fn live_international_elevation_sources() {
    let root = std::env::temp_dir().join(format!("1kee-provider-live-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let fixtures = [
        (Provider::England, 51.501, -0.128),
        (Provider::Netherlands, 52.371, 4.891),
        (Provider::France, 48.851, 2.351),
        (Provider::Switzerland, 46.945, 7.445),
        (Provider::NewZealand, -41.29, 174.775),
        (Provider::Japan, 35.681, 139.767),
    ];
    let filter = std::env::var("ONEKEE_ELEVATION_TEST_PROVIDER").unwrap_or_default();
    let mut failures = Vec::new();
    for (provider, lat, lon) in fixtures {
        if !filter.is_empty() && !format!("{provider:?}").eq_ignore_ascii_case(&filter) {
            continue;
        }
        let request = request(lat, lon);
        let output = root.join(format!("{provider:?}.tif"));
        let started = Instant::now();
        match fetch(
            request.bounds,
            request.width,
            request.height,
            &output,
            |_, _| {},
        ) {
            Ok(actual) => {
                assert_eq!(actual, provider);
                assert!(std::fs::metadata(&output).unwrap().len() > 1000);
                eprintln!(
                    "{} verified in {:.2}s",
                    provider.label(),
                    started.elapsed().as_secs_f64()
                );
            }
            Err(error) => {
                eprintln!("{provider:?}: {error:?}");
                failures.push((provider, error));
            }
        }
    }
    std::fs::remove_dir_all(root).unwrap();
    assert!(failures.is_empty(), "{failures:?}");
}
