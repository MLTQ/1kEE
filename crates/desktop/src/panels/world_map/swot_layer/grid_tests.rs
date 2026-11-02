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
        1024,
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
        1024,
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
            },
            1024
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

#[test]
fn globe_window_preserves_detail_beyond_old_1024_pixel_cap() {
    let mut bytes = vec![0u8; WIDTH * 2];
    for x in 0..2048 {
        let value: i16 = if x % 2 == 0 { -150 } else { 150 };
        bytes[x * 2..x * 2 + 2].copy_from_slice(&value.to_le_bytes());
    }
    let region = Region {
        x: 0,
        y: 0,
        width: 2048,
        height: 1,
    };
    let native = read_region(&mut Cursor::new(bytes.clone()), region, 4096).unwrap();
    assert_eq!(native.size, [2048, 1]);
    for (x, pixel) in native.pixels.iter().enumerate() {
        assert_eq!(*pixel, color(if x % 2 == 0 { -150 } else { 150 }));
    }
    let reduced = read_region(&mut Cursor::new(bytes), region, 1024).unwrap();
    assert_eq!(reduced.size, [1024, 1]);
    assert!(reduced.pixels.iter().all(|pixel| *pixel == color(150)));
}

#[test]
#[ignore = "read-only installed SWOT data; set ONEKEE_SWOT_ROOT; optional ONEKEE_SWOT_PREVIEW"]
fn installed_native_window_smoke() {
    let root =
        std::path::PathBuf::from(std::env::var_os("ONEKEE_SWOT_ROOT").expect("data directory"));
    let region = Region::from_bounds(-42.0, -25.0, -18.0, -9.0);
    let start = std::time::Instant::now();
    let image = read_image(&root, Some(region), 4096).unwrap();
    assert_eq!(image.size, [region.width, region.height]);
    assert!(image.pixels.windows(2).any(|p| p[0] != p[1]));
    eprintln!(
        "Native SWOT window {}x{} loaded in {:?}",
        image.size[0],
        image.size[1],
        start.elapsed()
    );
    if let Some(path) = std::env::var_os("ONEKEE_SWOT_PREVIEW") {
        let rgba: Vec<u8> = image.pixels.iter().flat_map(|p| p.to_array()).collect();
        image::save_buffer(
            path,
            &rgba,
            image.size[0] as u32,
            image.size[1] as u32,
            image::ColorType::Rgba8,
        )
        .unwrap();
    }
}
