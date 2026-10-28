//! Integration regressions for responsive chrome and settings navigation.
use super::*;
use crate::model::{
    ActiveBody, CameraConnectionState, CameraFeed, EventRecord, EventSeverity, FactalBrief,
    GeoPoint,
};
use egui::{Pos2, Rect};

fn model() -> AppModel {
    let mut model = AppModel::new();
    // Only synthetic UI fixtures; no poll/save action is invoked.
    model.factal_api_key = "test-key".into();
    // Match the drawer slider's step so rendering cannot round/save a legacy width.
    model.set_contour_stroke_width_px(2.0);
    let event = EventRecord {
        id: "preview".into(),
        title: "UI fixture".into(),
        summary: String::new(),
        severity: EventSeverity::Elevated,
        location_name: "Preview".into(),
        location: GeoPoint { lat: 0., lon: 0. },
        source: "Fixture".into(),
        occurred_at: String::new(),
        factal_brief: Some(FactalBrief {
            factal_id: "preview".into(),
            severity_value: None,
            occurred_at_raw: None,
            point_wkt: None,
            vertical: None,
            subvertical: None,
            topics: Vec::new(),
            content: None,
            raw_json_pretty: String::new(),
        }),
    };
    model.events = vec![event; 97];
    model.selected_event_id = Some("preview".into());
    model.cameras = vec![
        CameraFeed {
            id: "preview".into(),
            label: "UI fixture".into(),
            provider: String::new(),
            kind: String::new(),
            location: GeoPoint { lat: 0., lon: 0. },
            stream_url: String::new(),
            last_seen: String::new(),
            status: CameraConnectionState::Idle
        };
        1528
    ];
    model.camera_registry_progress_label =
        "Long camera discovery details must not expand the rail. ".repeat(20);
    model.show_stellar_correspondence = true;
    model
}
fn frame(
    ctx: &egui::Context,
    model: &mut AppModel,
    width: f32,
    events: Vec<egui::Event>,
) -> egui::FullOutput {
    frame_at_size(ctx, model, egui::vec2(width, 864.), events)
}
fn frame_at_size(
    ctx: &egui::Context,
    model: &mut AppModel,
    size: egui::Vec2,
    events: Vec<egui::Event>,
) -> egui::FullOutput {
    ctx.run(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
            events,
            ..Default::default()
        },
        |ctx| {
            if !model.obs_view {
                super::super::layer_drawer::render_layer_drawer(ctx, model);
            }
            egui::CentralPanel::default().show(ctx, |ui| {
                if !model.obs_view {
                    super::super::map_rail::render(ui, model);
                }
            });
            if !model.obs_view {
                render_settings(ctx, model);
            }
        },
    )
}
fn labels(output: &egui::FullOutput) -> Vec<(String, Rect)> {
    fn collect(shape: &egui::Shape, out: &mut Vec<(String, Rect)>) {
        match shape {
            egui::Shape::Text(text) => {
                out.push((text.galley.text().to_owned(), shape.visual_bounding_rect()))
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect(shape, out)
                }
            }
            _ => {}
        }
    }
    let mut result = Vec::new();
    for shape in &output.shapes {
        collect(&shape.shape, &mut result)
    }
    result
}
fn click(ctx: &egui::Context, model: &mut AppModel, width: f32, label: &str) -> egui::FullOutput {
    let output = frame(ctx, model, width, vec![]);
    let pos = labels(&output)
        .into_iter()
        .find(|(text, _)| text == label)
        .unwrap_or_else(|| panic!("Missing {label}"))
        .1
        .center();
    let event = |pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    frame(
        ctx,
        model,
        width,
        vec![egui::Event::PointerMoved(pos), event(true)],
    );
    frame(ctx, model, width, vec![event(false)]);
    frame(ctx, model, width, vec![])
}
fn context() -> egui::Context {
    let ctx = egui::Context::default();
    crate::theme::install(&ctx);
    ctx.style_mut(|style| style.animation_time = 0.0);
    ctx
}

#[test]
fn chrome_rail_stays_one_row_with_primary_controls_at_narrow_widths() {
    for width in [1536., 1100., 1090., 1080., 850., 540.] {
        for meander in [false, true] {
            let ctx = context();
            let mut model = model();
            model.globe_view.meander_mode = meander;
            frame(&ctx, &mut model, width, vec![]);
            let output = frame(&ctx, &mut model, width, vec![]);
            let labels = labels(&output);
            for name in [
                "GLOBE",
                "LOCAL",
                "EARTH",
                "Layers",
                "⚡ Cache Blast",
                "Settings",
            ] {
                assert!(
                    labels.iter().any(|(text, _)| text == name),
                    "{width}: missing {name}"
                );
            }
            assert!(
                labels
                    .iter()
                    .all(|(_, r)| r.left() >= 0. && r.right() <= width && r.top() < 55.),
                "{width}: {labels:?}"
            );
            for (i, (a, ra)) in labels.iter().enumerate() {
                for (b, rb) in labels.iter().skip(i + 1) {
                    assert!(!ra.intersects(*rb), "{width}: {a} overlaps {b}");
                }
            }
            assert!(!labels.iter().any(|(text, _)| text.contains("Long camera")));
            assert!(!labels.iter().any(|(text, _)| text == "CINEMATIC"));
            if width >= 1100. {
                for name in ["MEANDER", "FOLLOW EVENTS"] {
                    assert!(
                        labels.iter().any(|(text, _)| text == name),
                        "{width}: missing {name}"
                    );
                }
            }
        }
    }
}
#[test]
fn chrome_settings_and_embedded_terrain_remain_accessible() {
    let ctx = context();
    let mut model = model();
    frame(&ctx, &mut model, 1536., vec![]);
    click(&ctx, &mut model, 1536., "Settings");
    assert!(model.settings_open);
    let output = click(&ctx, &mut model, 1536., "Terrain Library");
    let text = labels(&output);
    assert!(text.iter().any(|(s, _)| s == "City Search"));
    assert_eq!(
        text.iter().filter(|(s, _)| s == "Terrain Library").count(),
        1,
        "library is embedded, not a second window"
    );
    let output = click(&ctx, &mut model, 1536., "Activity");
    assert!(
        labels(&output)
            .iter()
            .any(|(s, _)| s == "Recent source, import and cache activity.")
    );
    // Focus capture hides the window but preserves its state for restoration.
    model.obs_view = true;
    let output = frame(&ctx, &mut model, 1536., vec![]);
    assert!(labels(&output).is_empty());
    assert!(model.settings_open);
    model.obs_view = false;
    assert!(!labels(&frame(&ctx, &mut model, 1536., vec![])).is_empty());
}
#[test]
fn chrome_body_cycle_and_layer_import_keep_their_actions() {
    let ctx = context();
    let mut model = model();
    model.globe_view.local_mode = true;
    frame(&ctx, &mut model, 1100., vec![]);
    click(&ctx, &mut model, 1100., "EARTH");
    assert_eq!(model.active_body, ActiveBody::Moon);
    assert!(!model.globe_view.local_mode);
    assert_eq!(model.map_theme, theme::MapTheme::Lunar);
    click(&ctx, &mut model, 1100., "MOON");
    assert_eq!(model.active_body, ActiveBody::Mars);
    assert_eq!(model.map_theme, theme::MapTheme::MarsDark);
    click(&ctx, &mut model, 1100., "MARS");
    assert_eq!(model.active_body, ActiveBody::Earth);
    let output = click(&ctx, &mut model, 1100., "Layers");
    assert!(model.show_layer_drawer);
    assert!(labels(&output).iter().any(|(s, _)| s == "Import Layer…"));
}

#[test]
fn chrome_settings_footer_stays_visible_in_every_section() {
    for height in [720., 864.] {
        for (section, name) in Section::ALL {
            let ctx = context();
            let mut model = model();
            model.settings_open = true;
            ctx.data_mut(|d| d.insert_temp(egui::Id::new("settings-section"), section));
            let size = egui::vec2(1100., height);
            for _ in 0..4 {
                frame_at_size(&ctx, &mut model, size, vec![]);
            }
            let output = frame_at_size(&ctx, &mut model, size, vec![]);
            let save = output
                .shapes
                .iter()
                .find(|s| {
                    matches!(&s.shape,
                egui::Shape::Text(t) if t.galley.text() == "Save Settings")
                })
                .unwrap_or_else(|| panic!("{name}: missing Save Settings"));
            let rect = save.shape.visual_bounding_rect();
            assert!(
                save.clip_rect.contains_rect(rect),
                "{name}: clipped footer {rect:?} / {:?}",
                save.clip_rect
            );
            assert!(
                rect.bottom() < height,
                "{name}: footer below screen {rect:?}"
            );
        }
    }
}

#[path = "chrome_preview.rs"]
mod preview;

#[test]
fn chrome_camera_motion_controls_work_without_a_mode_toggle() {
    let ctx = context();
    let mut model = model();
    frame(&ctx, &mut model, 1536., vec![]);
    click(&ctx, &mut model, 1536., "MEANDER");
    assert!(model.globe_view.meander_mode);
    click(&ctx, &mut model, 1536., "FOLLOW EVENTS");
    assert!(model.event_follow.enabled());
    assert!(!model.globe_view.meander_mode);
    model.tick_event_follow(0.0);
    assert!(model.event_follow.enabled());
    assert!(model.factal_brief_open);
    click(&ctx, &mut model, 1536., "MEANDER");
    assert!(!model.event_follow.enabled());
    assert!(model.globe_view.meander_mode);
    click(&ctx, &mut model, 1536., "MEANDER");
    assert!(!model.globe_view.meander_mode);
}
