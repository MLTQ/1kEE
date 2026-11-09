use super::*;
#[path = "atlas_surface_gpu_tests.rs"]
mod gpu_tests;

#[test]
fn surface_shaders_validate_and_uniform_layout_matches() {
    use wgpu::naga;
    for text in [
        shader_source(),
        format!(
            "{}\n{}",
            include_str!("color_output.wgsl"),
            include_str!("atlas_surface_blit.wgsl")
        ),
    ] {
        let module = naga::front::wgsl::parse_str(&text).expect("surface shader parses");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("surface shader validates");
    }
    assert_eq!(std::mem::size_of::<Uniforms>(), 256);
}

#[test]
fn terrain_uses_the_contour_projection_and_layer_spread() {
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000., 700.));
    let layout = LocalLayout {
        center: rect.center(),
        focus_center: rect.center(),
        width: 820.,
        height: 518.,
        horizontal_scale: 310.,
    };
    let focus = GeoPoint { lat: 46., lon: 7.5 };
    let ctx = egui::Context::default();
    let painter = ctx
        .layer_painter(egui::LayerId::background())
        .with_clip_rect(rect);
    let data = Raster::new(
        HeightGrid {
            width: 4,
            height: 4,
            metres: vec![1000.; 16],
        },
        [7., 45.5, 8., 46.5],
    )
    .unwrap();
    for spread in [1., 12., 100.] {
        for pitch in [0.2, 1.0, 1.5] {
            let mut view = GlobeViewState::from_focus(focus);
            view.local_layer_spread = spread;
            view.local_pitch = pitch;
            let callback = Callback::local(
                &painter,
                &layout,
                &view,
                data.clone(),
                focus,
                0.5,
                true,
                false,
                1.,
            );
            let u = callback.uniforms[0];
            let p = local_projection_params(
                &layout,
                &view,
                focus,
                (0.5 * 111.32 * focus.lat.to_radians().cos()).max(1.),
                0.5 * 111.32,
            );
            for h in [-1500., 0., 4200.] {
                let point = GeoPoint {
                    lat: 46.1,
                    lon: 7.6,
                };
                let x = (point.lon - u.a[0]) * u.a[2];
                let y = (point.lat - u.a[1]) * u.a[3];
                let z = h * u.b[0];
                let xr = x * u.b[1] - y * u.b[2];
                let yr = x * u.b[2] + y * u.b[1];
                let screen = [
                    u.c[1] + xr * u.c[3],
                    u.c[2] - yr * u.b[3] * u.d[0] + yr * u.c[0] * u.d[1]
                        - z * u.c[0] * u.d[2]
                        - z * u.b[3] * u.d[3],
                ];
                let expected = p.project_logical(point, h);
                assert!(
                    (screen[0] - expected.0).abs() < 0.001
                        && (screen[1] - expected.1).abs() < 0.001
                );
            }
        }
    }
}

#[test]
fn local_surface_unwraps_data_beside_the_focus() {
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1024., 768.));
    let layout = LocalLayout {
        center: rect.center(),
        focus_center: rect.center(),
        width: 840.,
        height: 568.,
        horizontal_scale: 317.,
    };
    let focus = GeoPoint {
        lat: 20.,
        lon: -179.5,
    };
    let view = GlobeViewState::from_focus(focus);
    let ctx = egui::Context::default();
    let painter = ctx
        .layer_painter(egui::LayerId::background())
        .with_clip_rect(rect);
    let data = Raster::new(
        HeightGrid {
            width: 4,
            height: 4,
            metres: vec![2000.; 16],
        },
        [179., 19., 181., 21.],
    )
    .unwrap();
    let callback = Callback::local(&painter, &layout, &view, data, focus, 0.5, true, false, 1.0);
    assert_eq!(callback.uniforms[0].extra[1], -360.0);
    assert!(
        (callback.uniforms[0].region[0] + callback.uniforms[0].extra[1] - focus.lon).abs() < 2.0
    );
}
