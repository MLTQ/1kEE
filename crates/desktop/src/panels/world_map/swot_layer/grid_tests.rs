use super::*;
use std::io::Cursor;

#[test]
fn local_read_keeps_north_up_wraps_dateline_and_preserves_missing_values() {
    let mut data = vec![0u8; WIDTH * 4];
    for (x, y, value) in [
        (WIDTH - 1, 0, -200i16),
        (0, 0, 200),
        (WIDTH - 1, 1, NODATA),
        (0, 1, 0),
    ] {
        let at = (y * WIDTH + x) * 2;
        data[at..at + 2].copy_from_slice(&value.to_le_bytes());
    }
    let image = read_region(
        &mut Cursor::new(data),
        Region {
            x: -1,
            y: 0,
            width: 2,
            height: 2,
        },
    )
    .unwrap();
    assert_eq!(image.size, [2, 2]);
    assert_eq!(
        image.pixels,
        [
            color(-200),
            color(200),
            egui::Color32::TRANSPARENT,
            color(0)
        ]
    );
}

#[test]
fn polar_rows_are_transparent_and_truncation_is_an_error() {
    let image = read_region(
        &mut Cursor::new(Vec::<u8>::new()),
        Region {
            x: 0,
            y: -1,
            width: 2,
            height: 1,
        },
    )
    .unwrap();
    assert!(
        image
            .pixels
            .iter()
            .all(|p| *p == egui::Color32::TRANSPARENT)
    );
    assert!(
        read_region(
            &mut Cursor::new(vec![0; 2]),
            Region {
                x: 0,
                y: 0,
                width: 2,
                height: 1
            }
        )
        .is_err()
    );
}

#[test]
fn fixed_units_clamping_and_region_edges() {
    assert_eq!(color(-300), color(-200));
    assert_eq!(color(300), color(200));
    assert_ne!(color(20), color(200));
    assert_eq!(color(NODATA).a(), 0);
    let region = Region::around(0.0, 179.8, 0.1);
    assert_eq!(region, Region::around(0.01, 179.81, 0.1));
    let [west, south, east, north] = region.bounds();
    assert!(west < 179.8 && east > 180.0 && south < 0.0 && north > 0.0);
}
