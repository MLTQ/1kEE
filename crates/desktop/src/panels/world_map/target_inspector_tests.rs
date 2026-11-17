//! Exercise picking and the measured profile through the visible Target panel.
use super::*;
#[path = "admin_test_screen.rs"]
mod screen;

#[test]
#[ignore = "requires GPU and ONEKEE_TARGET_TEST_CATALOG with the retained Israel DEM"]
fn inspect_real_terrain_with_two_pointer_clicks() {
    let catalog = std::path::PathBuf::from(std::env::var_os("ONEKEE_TARGET_TEST_CATALOG").unwrap());
    let marker = tempfile::NamedTempFile::new().unwrap();
    let region = Region {
        lon: 34.84,
        lat: 32.30,
        radius_m: 1000.,
    };
    let dem =
        crate::target_mode::tests::live_local_terrain(&catalog, region, marker.path()).unwrap();
    // Use actual triangle interiors on both sides of the target.
    let point = |x: f32| {
        dem.indices
            .as_chunks::<3>()
            .0
            .iter()
            .map(|tri| {
                std::array::from_fn::<_, 3, _>(|d| {
                    tri.iter()
                        .map(|i| dem.batch.points[*i as usize].position[d])
                        .sum::<f32>()
                        / 3.
                })
            })
            .min_by(|a, b| ((a[0] - x).hypot(a[1])).total_cmp(&(b[0] - x).hypot(b[1])))
            .unwrap()
    };
    let ends = [point(-550.), point(550.)];
    let mut model = AppModel::new();
    let mut session = crate::target_mode::tests::session();
    session.region = region;
    session.camera.reset(dem.center_height());
    session.show_terrain = true;
    session.show_points = false;
    session.show_context = false;
    session.profile.enabled = true;
    session.terrain = Some(std::sync::Arc::new(dem));
    session.terrain_status = session.terrain.as_ref().unwrap().label.clone();
    session.context_status.clear();
    session.status.clear();
    model.target.session = Some(session);
    model.target.radius_km = 1.;
    let ctx = egui::Context::default();
    let mut screen = screen::Screen::new([1280, 896]);
    screen.install_target();
    let mut frame = |model: &mut AppModel, events, capture| {
        let mut rect = egui::Rect::NOTHING;
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280., 896.),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    rect = render(ui, model);
                });
            },
        );
        (rect, screen.render(&ctx, output, capture))
    };
    for end in ends {
        let (rect, _) = frame(&mut model, vec![], false);
        let pos = model
            .target
            .session
            .as_ref()
            .unwrap()
            .camera
            .project(end, rect)
            .unwrap();
        for pressed in [true, false] {
            frame(
                &mut model,
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ],
                false,
            );
        }
    }
    let [lo, hi] = model
        .target
        .session
        .as_ref()
        .unwrap()
        .profile
        .measured_range()
        .expect("clicked a measured transect");
    assert!(hi - lo > 0.1);
    let (_, rgba) = frame(&mut model, vec![], true);
    if let Some(path) = std::env::var_os("ONEKEE_TARGET_INSPECTOR_IMAGE") {
        image::save_buffer(path, &rgba.unwrap(), 1280, 896, image::ColorType::Rgba8).unwrap();
    }
}
