use super::*;

#[test]
fn local_button_enters_the_local_renderer_without_a_selected_event() {
    let ctx = egui::Context::default();
    let mut model = AppModel::new();
    model.events.clear();
    model.selected_event_id = None;
    model.focused_city_id = None;
    model.globe_view.local_mode = false;
    model.globe_view.focus_on(crate::model::GeoPoint {
        lat: 48.85,
        lon: 2.35,
    });
    let center = model.globe_view.globe_center_latlon();
    let frame = |model: &mut AppModel, events| {
        ctx.run(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.horizontal(|ui| view_buttons(ui, model));
                });
            },
        )
    };
    let output = frame(&mut model, Vec::new());
    let local = output
        .shapes
        .iter()
        .find_map(|s| match &s.shape {
            egui::Shape::Text(t) if t.galley.text() == "LOCAL" => {
                Some(t.pos + t.galley.size() / 2.0)
            }
            _ => None,
        })
        .expect("Local control");
    for pressed in [true, false] {
        let _ = frame(
            &mut model,
            vec![
                egui::Event::PointerMoved(local),
                egui::Event::PointerButton {
                    pos: local,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Default::default(),
                },
            ],
        );
    }
    assert!(model.globe_view.local_mode);
    assert_eq!(model.globe_view.local_center, center);
}

#[test]
fn top_bar_keeps_camera_control_and_has_no_lidar_control() {
    let ctx = egui::Context::default();
    let mut model = AppModel::new();
    let frame = |model: &mut AppModel, events| {
        ctx.run(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.horizontal(|ui| record_buttons(ui, model));
                });
            },
        )
    };
    assert!(!model.lidar.enabled && !model.show_camera_markers);
    for cameras in [true, false] {
        let label = "Cameras 0";

        let out = frame(&mut model, vec![]);
        assert!(!out.shapes.iter().any(
            |shape| matches!(&shape.shape,egui::Shape::Text(t) if t.galley.text().contains("Lidar"))
        ));
        let pos = out
            .shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.text() == label => {
                    Some(t.pos + t.galley.size() / 2.0)
                }
                _ => None,
            })
            .expect("Earth layer control");
        for pressed in [true, false] {
            let _ = frame(
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
            );
        }
        assert_eq!(
            (model.lidar.enabled, model.show_camera_markers),
            (false, cameras)
        );
    }
}
