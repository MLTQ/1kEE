use super::*;

#[test]
fn windows_keep_native_detail_and_bound_texture_memory() {
    let region = Region([0, 0, 7200, 3600]);
    assert_eq!(region.size(15.0, 8192), [480, 240]);
    assert_eq!(region.size(1.0, 8192), [7200, 3600]);
    let huge = Region([-648000, -324000, 648000, 324000]);
    let size = huge.size(1.0, 8192);
    assert!(size[0] <= 8192 && size[1] <= 8192 && size[0] * size[1] <= 32_000_000);
    assert!(huge.size(15.0, 2048).iter().all(|&v| v <= 2048));
}

#[test]
fn deep_local_windows_follow_the_rotated_clip_and_cross_the_dateline() {
    let clip = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280., 800.));
    let layout = LocalLayout {
        center: clip.center(),
        focus_center: clip.center() + egui::vec2(18., 12.),
        width: 1050.,
        height: 592.,
        horizontal_scale: 397.,
    };
    for lon in [-179.99, 179.99] {
        let focus = GeoPoint { lat: 46., lon };
        let mut view = GlobeViewState::from_focus(focus);
        view.local_yaw = 1.1;
        view.local_pitch = 1.4;
        let region = local(clip, &layout, &view, focus, 0.01).unwrap();
        let b = region.bounds();
        assert!(b[0] < lon && b[2] > lon);
        assert!(
            b[2] - b[0] < 1.0,
            "fine zoom never requests an entire longitude degree"
        );
        assert!(b[1] < focus.lat && b[3] > focus.lat);
        let mesh = super::super::raster_mesh::local(
            &layout,
            &view,
            egui::TextureId::Managed(0),
            b,
            focus,
            0.01,
            clip,
        );
        assert_eq!(mesh.vertices.len(), 4);
        assert!(
            mesh.vertices
                .iter()
                .all(|v| v.uv.x >= 0.0 && v.uv.x <= 1.0 && v.uv.y >= 0.0 && v.uv.y <= 1.0)
        );
    }
}

#[test]
fn globe_detail_uses_the_real_zoom_and_visible_pole() {
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280., 800.));
    let mut view = GlobeViewState::from_focus(GeoPoint { lat: 46., lon: 7.5 });
    view.zoom = 17.;
    let layout = super::super::globe_scene::globe_layout(rect, &view);
    let b = globe(rect, &layout, &view, 2.0).unwrap().bounds();
    assert!(b[0] < 7.5 && b[2] > 7.5 && b[1] < 46.0 && b[3] > 46.0);
    assert!(b[2] - b[0] < 90.0);
    view = GlobeViewState::from_focus(GeoPoint { lat: 90., lon: 0. });
    view.zoom = 1.0;
    let layout = super::super::globe_scene::globe_layout(rect, &view);
    let b = globe(rect, &layout, &view, 2.0).unwrap().bounds();
    assert_eq!(b[3], 90.0);
    assert_eq!(b[2] - b[0], 360.0);
}

#[test]
fn padded_windows_contain_date_line_equivalent_views() {
    let region = Region([179 * 3600, -3600, 181 * 3600, 3600]);
    assert!(region.contains([-180.5, -0.5, -179.5, 0.5]));
    assert!(!region.contains([-180.5, -0.5, -178.5, 0.5]));
    assert!(!region.contains([179.5, -0.5, f32::NAN, 0.5]));
}
