use super::*;

fn layout() -> LocalLayout {
    LocalLayout {
        center: egui::pos2(800.0, 450.0),
        focus_center: egui::pos2(832.0, 462.0),
        width: 1312.0,
        height: 666.0,
        horizontal_scale: 496.0,
    }
}

#[test]
fn large_regional_image_covers_deep_local_zoom_without_vertex_culling() {
    let focus = GeoPoint {
        lat: 40.0,
        lon: -100.0,
    };
    let mut view = GlobeViewState::from_focus(focus);
    let clip = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 900.0));
    for half in [5.0, 0.1, 0.001] {
        for yaw in [0.0, 1.3] {
            for pitch in [0.4, 1.55] {
                view.local_yaw = yaw;
                view.local_pitch = pitch;
                let mesh = local(
                    &layout(),
                    &view,
                    egui::TextureId::Managed(1),
                    [-130.0, 20.0, -60.0, 55.0],
                    focus,
                    half,
                    clip,
                );
                assert_eq!(mesh.indices.len(), 6);
                assert!(mesh.vertices.iter().all(|v| v.pos.is_finite()
                    && (0.0..=1.0).contains(&v.uv.x)
                    && (0.0..=1.0).contains(&v.uv.y)));
                if half <= 0.001 || (half <= 0.1 && pitch < 1.0) {
                    // Each canvas corner lies inside the projected geographic AABB.
                    for corner in [
                        clip.left_top(),
                        clip.right_top(),
                        clip.left_bottom(),
                        clip.right_bottom(),
                    ] {
                        let positions: Vec<_> = mesh.vertices.iter().map(|v| v.pos).collect();
                        let inside = |a: egui::Pos2, b: egui::Pos2, c: egui::Pos2| {
                            let cross = |a: egui::Pos2, b: egui::Pos2| {
                                let a = a - corner;
                                let b = b - corner;
                                a.x * b.y - a.y * b.x
                            };
                            let signs = [cross(a, b), cross(b, c), cross(c, a)];
                            signs.iter().all(|s| *s >= -0.1) || signs.iter().all(|s| *s <= 0.1)
                        };
                        assert!(
                            inside(positions[0], positions[1], positions[2])
                                || inside(positions[0], positions[2], positions[3])
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn alaska_wraps_to_either_side_of_dateline_and_outside_region_is_empty() {
    let clip = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 900.0));
    for lon in [-183.0, 177.0] {
        let focus = GeoPoint { lat: 60.0, lon };
        let view = GlobeViewState::from_focus(focus);
        let mesh = local(
            &layout(),
            &view,
            egui::TextureId::Managed(1),
            [-188.0, 49.0, -126.0, 72.0],
            focus,
            0.1,
            clip,
        );
        assert_eq!(mesh.indices.len(), 6);
    }
    let focus = GeoPoint {
        lat: -40.0,
        lon: 0.0,
    };
    let view = GlobeViewState::from_focus(focus);
    assert!(
        local(
            &layout(),
            &view,
            egui::TextureId::Managed(1),
            [-130.0, 20.0, -60.0, 55.0],
            focus,
            0.1,
            clip
        )
        .indices
        .is_empty()
    );
}

#[test]
fn global_strip_shows_both_texture_edges_at_dateline() {
    let clip = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 900.0));
    let focus = GeoPoint {
        lat: 10.0,
        lon: 180.0,
    };
    let mesh = local(
        &layout(),
        &GlobeViewState::from_focus(focus),
        egui::TextureId::Managed(1),
        [-180.0, -90.0, 180.0, 90.0],
        focus,
        1.0,
        clip,
    );
    assert_eq!(mesh.indices.len(), 12);
    assert!(mesh.vertices.iter().any(|v| v.uv.x == 0.0));
    assert!(mesh.vertices.iter().any(|v| v.uv.x == 1.0));
}
