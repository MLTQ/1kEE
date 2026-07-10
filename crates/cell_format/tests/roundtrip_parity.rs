use cell_format::{
    CellFeature, CellPoint, TAG_ROAD, TAG_WATR, read::read_single_chunk, write::write_cell,
};

fn assert_features_bit_exact(expected: &[CellFeature], actual: &[CellFeature]) {
    assert_eq!(expected.len(), actual.len());

    for (expected, actual) in expected.iter().zip(actual) {
        assert_eq!(expected.way_id, actual.way_id);
        assert_eq!(expected.class, actual.class);
        assert_eq!(expected.is_polygon, actual.is_polygon);
        assert_eq!(expected.name, actual.name);
        assert_eq!(expected.points.len(), actual.points.len());

        for (expected_point, actual_point) in expected.points.iter().zip(&actual.points) {
            assert_eq!(expected_point.lon.to_bits(), actual_point.lon.to_bits());
            assert_eq!(expected_point.lat.to_bits(), actual_point.lat.to_bits());
        }

        match (&expected.elevations, &actual.elevations) {
            (None, None) => {}
            (Some(expected_elevations), Some(actual_elevations)) => {
                assert_eq!(expected_elevations.len(), actual_elevations.len());
                for (expected_elevation, actual_elevation) in
                    expected_elevations.iter().zip(actual_elevations)
                {
                    assert_eq!(expected_elevation.to_bits(), actual_elevation.to_bits());
                }
            }
            _ => panic!("elevation presence changed during round trip"),
        }
    }
}

#[test]
fn cell_round_trip_preserves_feature_order_and_float_bits() {
    let roads = vec![
        CellFeature {
            way_id: 7,
            class: 2,
            is_polygon: false,
            name: Some("Alpha Route".into()),
            points: vec![
                CellPoint {
                    lon: -73.987_654,
                    lat: 40.765_432,
                },
                CellPoint {
                    lon: -73.912_345,
                    lat: 40.712_345,
                },
            ],
            elevations: None,
        },
        CellFeature {
            way_id: 11,
            class: 5,
            is_polygon: false,
            name: None,
            points: vec![
                CellPoint {
                    lon: 179.875,
                    lat: -11.25,
                },
                CellPoint {
                    lon: -179.5,
                    lat: -11.125,
                },
            ],
            elevations: Some(vec![0.0, f32::from_bits(0x8000_0000)]),
        },
    ];
    let waterways = vec![CellFeature {
        way_id: 21,
        class: 1,
        is_polygon: false,
        name: Some("Delta Run".into()),
        points: vec![CellPoint {
            lon: 12.5,
            lat: 54.25,
        }],
        elevations: Some(vec![f32::from_bits(0x7fc0_0042)]),
    }];

    let bytes = write_cell(40, -74, &[(TAG_ROAD, &roads), (TAG_WATR, &waterways)]);

    let decoded_roads = read_single_chunk(&bytes, TAG_ROAD).expect("road chunk should decode");
    let decoded_waterways =
        read_single_chunk(&bytes, TAG_WATR).expect("waterway chunk should decode");

    assert_features_bit_exact(&roads, &decoded_roads);
    assert_features_bit_exact(&waterways, &decoded_waterways);
}
