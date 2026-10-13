use super::*;

fn seattle() -> Bounds {
    Bounds {
        min_lat: 47.59,
        min_lon: -122.36,
        max_lat: 47.63,
        max_lon: -122.32,
    }
}

#[test]
fn washington_products_request_distinct_transparent_geographic_layers() {
    for (source, layer) in [
        (Source::WashingtonBareEarth, "show:1"),
        (Source::WashingtonSurface, "show:0"),
    ] {
        let url = washington_url(source, seattle(), 256, 256).unwrap();
        assert_eq!(url.host_str(), Some("gis.dnr.wa.gov"));
        let params = url
            .query_pairs()
            .collect::<std::collections::HashMap<_, _>>();
        assert_eq!(params["layers"], layer);
        assert_eq!(params["bbox"], "-122.36,47.59,-122.32,47.63");
        assert_eq!(params["bboxSR"], "4326");
        assert_eq!(params["imageSR"], "4326");
        assert_eq!(params["size"], "256,256");
        assert_eq!(params["format"], "png32");
        assert_eq!(params["transparent"], "true");
    }
}

#[test]
fn washington_skips_uncovered_invalid_and_oversized_requests() {
    let source = Source::WashingtonBareEarth;
    let new_york = Bounds::around(
        GeoPoint {
            lat: 40.7,
            lon: -74.0,
        },
        0.1,
    );
    assert!(!source.covers(new_york));
    assert!(Source::Usgs.covers(new_york));
    assert!(washington_url(source, new_york, 256, 256).is_none());
    assert!(washington_url(source, seattle(), u32::MAX, 256).is_none());
    assert!(washington_url(source, seattle(), 256, 0).is_none());
    for invalid in [
        Bounds {
            min_lat: f32::NAN,
            ..seattle()
        },
        Bounds {
            min_lon: -120.0,
            ..seattle()
        },
        Bounds {
            max_lat: f32::INFINITY,
            ..seattle()
        },
    ] {
        assert!(washington_url(source, invalid, 256, 256).is_none());
    }
    // An image crossing the state/service edge can still contain useful data.
    assert!(source.covers(Bounds {
        min_lon: -125.1,
        max_lon: -124.8,
        ..seattle()
    }));
}

#[test]
#[ignore = "downloads two small Seattle images from the public DNR service"]
fn live_washington_hillshade() {
    assert!(
        crate::threedep::is_enabled(),
        "enable detailed terrain streaming for this live test"
    );
    let mut products = Vec::new();
    for source in [Source::WashingtonBareEarth, Source::WashingtonSurface] {
        let bytes = source.fetch(seattle(), 256, 256).expect("DNR PNG response");
        let image = image::load_from_memory(&bytes)
            .expect("valid PNG")
            .to_rgba8();
        assert_eq!(image.dimensions(), (256, 256));
        let values: std::collections::HashSet<_> = image
            .pixels()
            .filter(|pixel| pixel.0[3] != 0)
            .map(|pixel| pixel.0[0])
            .collect();
        assert!(values.len() > 16, "expected varied visible relief");
        eprintln!(
            "{}: {} bytes, {} visible shade levels",
            source.label(),
            bytes.len(),
            values.len()
        );
        products.push(image);
    }
    assert_ne!(
        products[0], products[1],
        "bare earth and surface should differ in Seattle"
    );
}
