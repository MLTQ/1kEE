use super::*;

#[test]
fn rail_exposes_persp_ortho_and_no_legacy_globe_local_target_buttons() {
    let ctx = egui::Context::default();
    let mut model = AppModel::new();
    for body in [ActiveBody::Earth, ActiveBody::Moon, ActiveBody::Mars] {
        model.active_body = body;
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.horizontal(|ui| view_buttons(ui, &mut model));
            });
        });
        let has = |label: &str| {
            output
                .shapes
                .iter()
                .any(|s| matches!(&s.shape, egui::Shape::Text(t) if t.galley.text() == label))
        };
        assert!(has("PERSP"), "PERSP missing on {body:?}");
        assert!(has("ORTHO"), "ORTHO missing on {body:?}");
        assert!(has("N↑"), "N↑ missing on {body:?}");
        assert!(!has("GLOBE"), "legacy GLOBE button should be removed");
        assert!(!has("LOCAL"), "legacy LOCAL button should be removed");
        assert!(!has("TARGET"), "legacy TARGET button should be removed");
    }
}

#[test]
fn ortho_button_toggles_unified_projection_mode() {
    let ctx = egui::Context::default();
    let mut model = AppModel::new();
    assert_eq!(
        model.globe_view.projection,
        crate::model::ProjectionMode::Perspective
    );
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
    let ortho = output
        .shapes
        .iter()
        .find_map(|s| match &s.shape {
            egui::Shape::Text(t) if t.galley.text() == "ORTHO" => {
                Some(t.pos + t.galley.size() / 2.0)
            }
            _ => None,
        })
        .expect("ORTHO control");
    for pressed in [true, false] {
        let _ = frame(
            &mut model,
            vec![
                egui::Event::PointerMoved(ortho),
                egui::Event::PointerButton {
                    pos: ortho,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Default::default(),
                },
            ],
        );
    }
    assert_eq!(
        model.globe_view.projection,
        crate::model::ProjectionMode::Orthographic
    );
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
