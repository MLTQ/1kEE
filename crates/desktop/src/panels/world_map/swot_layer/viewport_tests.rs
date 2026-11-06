use super::super::super::globe_scene::globe_layout;
use super::*;

fn inputs(lat: f32, lon: f32, zoom: f32) -> (egui::Rect, GlobeViewState, GlobeLayout) {
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1536.0, 864.0));
    let mut view = GlobeViewState::from_focus(GeoPoint { lat, lon });
    view.zoom = zoom;
    let layout = globe_layout(rect, &view);
    (rect, view, layout)
}

#[test]
fn magnification_refines_and_deep_zoom_fits_native_texels() {
    let (rect, view, layout) = inputs(0.0, -30.0, 0.6);
    assert!(detail_region(rect, &layout, &view, 1.0).is_none());
    let (rect, view, layout) = inputs(0.0, -30.0, 50.0);
    let region = detail_region(rect, &layout, &view, 1.0).unwrap();
    assert!(
        region.width < 4096 && region.height < 4096,
        "Deep zoom must retain every native pixel: {region:?}"
    );
    // A dense screen sample must fall within the loaded image's geographic bounds.
    assert_covers_canvas(rect, &view, region);
}

fn assert_covers_canvas(rect: egui::Rect, view: &GlobeViewState, region: Region) {
    let [west, south, east, north] = region.bounds();
    for y in 0..=32 {
        for x in 0..=32 {
            let pos = rect.min
                + egui::vec2(
                    rect.width() * x as f32 / 32.0,
                    rect.height() * y as f32 / 32.0,
                );
            if let Some(point) = screen_to_latlon(rect, view, pos) {
                let lon = west + (point.lon - west).rem_euclid(360.0);
                assert!(
                    lon <= east + 0.001 && point.lat >= south - 0.001 && point.lat <= north + 0.001,
                    "{point:?} outside {region:?}"
                );
            }
        }
    }
}

#[test]
fn date_line_polar_and_partial_limb_views_stay_covered() {
    for (lat, lon, zoom) in [
        (0.0, 179.9, 50.0),
        (75.0, -179.9, 50.0),
        (85.0, 20.0, 4.0),
        (-85.0, 20.0, 4.0),
        (30.0, 40.0, 3.0),
    ] {
        let (rect, view, layout) = inputs(lat, lon, zoom);
        let region = detail_region(rect, &layout, &view, 1.0).unwrap();
        assert!(region.width <= super::super::grid::WIDTH);
        assert!(region.height <= super::super::grid::HEIGHT);
        assert_covers_canvas(rect, &view, region);
        if lat == 0.0 {
            assert!(
                region.width < 4096,
                "Dateline cannot widen a close view to the world"
            );
        }
    }
}

#[test]
fn physical_pixel_density_changes_refinement_threshold() {
    let (rect, view, layout) = inputs(0.0, 0.0, 1.0);
    assert!(detail_region(rect, &layout, &view, 1.0).is_none());
    assert!(detail_region(rect, &layout, &view, 2.0).is_some());
}
