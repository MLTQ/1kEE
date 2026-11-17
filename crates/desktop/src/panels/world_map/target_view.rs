//! Local selection cylinder and the independent Target flight viewport.
use super::{local_terrain_scene as local, target_pass};
use crate::{
    model::{AppModel, GeoPoint},
    target_mode::{ContextInput, Session},
    theme,
};
use lidar_pipeline::region::Region;

pub(super) fn selection(
    painter: &egui::Painter,
    rect: egui::Rect,
    model: &mut AppModel,
    response: &egui::Response,
) {
    selection_with(painter, rect, model, response, |model, region, ctx| {
        let input = ContextInput::capture(model, region);
        let catalog = crate::lidar_catalog::db_path(
            Some(&model.settings_data_root),
            model.selected_root.as_deref(),
        );
        Session::new(region, catalog, model.selected_root.clone(), input, ctx)
    });
}
fn selection_with(
    painter: &egui::Painter,
    rect: egui::Rect,
    model: &mut AppModel,
    response: &egui::Response,
    open: impl FnOnce(&AppModel, Region, &egui::Context) -> Result<Session, String>,
) {
    painter.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
    let center = model.globe_view.local_center;
    let half = local::visual_half_extent_for_zoom(model.globe_view.local_zoom);
    let projection = local::projection::local_projection_params(
        &local::layout(rect),
        &model.globe_view,
        center,
        (half * 111.32 * center.lat.to_radians().cos().abs().max(0.2)).max(1.),
        (half * 111.32).max(1.),
    );
    if let Some(pixel) = response.hover_pos()
        && let Some(mut p) = projection.ground_location(pixel)
    {
        // Invert the same oblique projection at cached terrain height so the
        // cylinder follows the cursor across hills rather than at sea level.
        for _ in 0..3 {
            let z = model
                .selected_root
                .as_ref()
                .and_then(|root| super::srtm_stream::peek_elevation_m(Some(root), p))
                .unwrap_or(0.);
            let (x0, y0, _) = projection.project_logical(p, 0.);
            let (x1, y1, _) = projection.project_logical(p, z);
            if let Some(q) = projection.ground_location(pixel - egui::vec2(x1 - x0, y1 - y0)) {
                p = q;
            }
        }
        model.target.hover = Some(Region {
            lon: p.lon.into(),
            lat: p.lat.into(),
            radius_m: model.target.radius_km.clamp(0.1, 100.) * 1000.,
        });
    }
    if let Some(mut region) = model.target.hover {
        region.radius_m = model.target.radius_km.clamp(0.1, 100.) * 1000.;
        model.target.hover = Some(region);
        let ground = model
            .selected_root
            .as_ref()
            .and_then(|root| {
                super::srtm_stream::peek_elevation_m(
                    Some(root),
                    GeoPoint {
                        lon: region.lon as f32,
                        lat: region.lat as f32,
                    },
                )
            })
            .unwrap_or(0.);
        let project = |ll: [f64; 2], z| {
            let (x, y, _) = projection.project_logical(
                GeoPoint {
                    lon: ll[0] as f32,
                    lat: ll[1] as f32,
                },
                z,
            );
            egui::pos2(x, y)
        };
        let red = egui::Color32::from_rgb(232, 57, 86);
        let mut base = Vec::new();
        let mut top = Vec::new();
        for i in 0..=96 {
            let a = i as f64 * std::f64::consts::TAU / 96.;
            let ll = region.location(a.sin() * region.radius_m, a.cos() * region.radius_m);
            base.push(project(ll, ground));
            top.push(project(ll, ground + 800.));
        }
        for i in (0..96).step_by(8) {
            painter.line_segment(
                [base[i], top[i]],
                egui::Stroke::new(1.0_f32, red.gamma_multiply(0.3)),
            );
        }
        painter.add(egui::Shape::line(
            top,
            egui::Stroke::new(1.0_f32, red.gamma_multiply(0.4)),
        ));
        painter.add(egui::Shape::line(base, egui::Stroke::new(2.0_f32, red)));
        let p = project([region.lon, region.lat], ground);
        painter.circle_stroke(p, 4., egui::Stroke::new(1.5_f32, red));
        painter.line_segment(
            [p - egui::vec2(8., 0.), p + egui::vec2(8., 0.)],
            egui::Stroke::new(1.0_f32, red),
        );
        painter.line_segment(
            [p - egui::vec2(0., 8.), p + egui::vec2(0., 8.)],
            egui::Stroke::new(1.0_f32, red),
        );
        painter.text(
            rect.left_top() + egui::vec2(18., 18.),
            egui::Align2::LEFT_TOP,
            format!(
                "TARGET · {:.1} KM RADIUS\n{:.6}, {:.6}\nWHEEL TO ZOOM · LEFT CLICK TO ENTER · ESC TO CANCEL",
                region.radius_m / 1000.,
                region.lat,
                region.lon
            ),
            egui::FontId::monospace(13.),
            red,
        );
        if response.clicked_by(egui::PointerButton::Primary) {
            match open(model, region, painter.ctx()) {
                Ok(session) => {
                    model.stop_event_follow();
                    model.target.session = Some(session);
                    model.target.selecting = false;
                    painter.ctx().request_repaint();
                }
                Err(error) => {
                    painter.text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        error,
                        egui::FontId::proportional(14.),
                        red,
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn target_selection_click_enters_flight_and_escape_preserves_the_local_camera() {
        let ctx = egui::Context::default();
        let mut model = AppModel::new();
        model.selected_root = None;
        model.globe_view.local_mode = true;
        model.globe_view.local_center = GeoPoint {
            lat: 35.26,
            lon: -77.58,
        };
        model.globe_view.local_zoom = 40.;
        model.globe_view.stop_motion();
        model.target.selecting = true;
        model.target.radius_km = 100.;
        let before = format!("{:?}", model.globe_view);
        let pointer = egui::pos2(400., 300.);
        for pressed in [None, Some(true), Some(false)] {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1000., 800.),
                    )),
                    events: vec![egui::Event::PointerMoved(pointer)]
                        .into_iter()
                        .chain(pressed.map(|pressed| egui::Event::PointerButton {
                            pos: pointer,
                            pressed,
                            button: egui::PointerButton::Primary,
                            modifiers: Default::default(),
                        }))
                        .collect(),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let (response, painter) =
                            ui.allocate_painter(ui.available_size(), egui::Sense::click_and_drag());
                        selection_with(
                            &painter,
                            response.rect,
                            &mut model,
                            &response,
                            |_, region, _| {
                                let mut session = crate::target_mode::tests::session();
                                session.region = region;
                                Ok(session)
                            },
                        );
                    });
                },
            );
        }
        let session = model
            .target
            .session
            .as_mut()
            .expect("left click enters Target");
        assert_eq!(session.region.radius_m, 100_000.);
        assert!(!model.target.selecting);
        session.camera.looking = true;
        let eye = session.camera.eye;
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1200., 900.),
                )),
                events: vec![egui::Event::Key {
                    key: egui::Key::W,
                    physical_key: Some(egui::Key::W),
                    pressed: true,
                    repeat: false,
                    modifiers: Default::default(),
                }],
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    super::super::render_world_map(ui, &mut model);
                });
            },
        );
        assert_ne!(model.target.session.as_ref().unwrap().camera.eye, eye);
        assert_eq!(format!("{:?}", model.globe_view), before);
        let _ = ctx.run(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key: egui::Key::Escape,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: Default::default(),
                }],
                ..Default::default()
            },
            |ctx| {
                assert!(model.target.consume_escape(ctx));
            },
        );
        assert!(!model.target.engaged());
        assert_eq!(model.target.radius_km, 100.);
        assert_eq!(format!("{:?}", model.globe_view), before);
    }
}

pub(super) fn render(ui: &mut egui::Ui, model: &mut AppModel) -> egui::Rect {
    let mut session = model.target.session.take().unwrap();
    session.poll(ui.ctx());
    let mut exit = false;
    let mut reset = false;
    let mut fly = false;
    let mut reload = false;
    let mut apply_radius = false;
    let mut radius_km = model.target.radius_km;
    let prior_source = session.selected_source;
    egui::Frame::new().fill(theme::panel_fill(230)).corner_radius(8_u8).inner_margin(8.0_f32).show(ui,|ui| {
        ui.horizontal_wrapped(|ui| {
            exit=ui.button("← LOCAL  [Esc]").clicked();
            ui.strong("TARGET");ui.label(format!("{:.1} km radius",session.region.radius_m/1000.));
            crate::target_mode::TargetMode::radius_control(ui,&mut radius_km);
            if (radius_km*1000.-session.region.radius_m).abs()>0.01 {
                apply_radius=ui.button("Apply radius").clicked();
            }
            ui.monospace(format!("{:.6}, {:.6}",session.region.lat,session.region.lon));
            reset=ui.button("Home").on_hover_text("Reset Target camera").clicked();
            fly=ui.button(if session.camera.looking {"Flying · RMB to release"}else{"Fly"}).clicked();
            ui.add(egui::Slider::new(&mut session.camera.speed,1. ..=10_000.).logarithmic(true).text("speed"));
        });
        ui.horizontal_wrapped(|ui| {
            if ui.checkbox(&mut session.show_points,"Point cloud").changed() && session.show_points && !session.compatible_dem() {
                session.show_terrain=false;reset=true;
            }
            if ui.checkbox(&mut session.show_terrain,"DEM terrain").changed() && session.show_terrain && !session.compatible_dem() {
                session.show_points=false;reset=true;
            }
            ui.checkbox(&mut session.show_context,"Map context");
            egui::ComboBox::from_id_salt("target-colors").selected_text(["Classification","Height","Source RGB","Intensity"][session.color_mode as usize])
                .show_ui(ui,|ui| {for (i,label) in ["Classification","Height","Source RGB","Intensity"].into_iter().enumerate() {
                    ui.selectable_value(&mut session.color_mode,i as u32,label);
                }});
            ui.add(egui::Slider::new(&mut session.point_size,0.5..=5.).text("point size"));
            ui.menu_button("Classes",|ui|class_controls(ui,&mut session.class_mask));
        });
        ui.horizontal_wrapped(|ui| {
            egui::ComboBox::from_id_salt("target-survey").width(340.)
                .selected_text(session.survey_label().to_owned())
                .show_ui(ui,|ui| {for (i,source) in session.sources.iter().enumerate() {
                    ui.selectable_value(&mut session.selected_source,i,format!("{} · {}",source.name,source.date));
                }});
            ui.add(egui::Slider::new(&mut session.budget_millions,1..=16).text("million points"));
            reload=ui.button("Refine / reload").clicked();
            ui.small(&session.status);
            if session.loading() {ui.small(format!("{:.0}s",session.started.elapsed().as_secs_f64()));}
        });
        ui.small(&session.terrain_status);ui.small(&session.context_status);
        if let Some(source)=&session.source {
            let vertical=source.crs.get("vertical").map(|v|v.to_string()).unwrap_or_else(||"undeclared".into());
            ui.horizontal_wrapped(|ui| {
                ui.hyperlink_to(format!("{} · {}",source.source.name,source.source.date),&source.publisher);
                ui.small(format!("Vertical reference: {vertical} · original source Z{}",
                    if vertical=="undeclared" {" · Z units unverified"} else {" · no height offsets"}));
            });
            if !session.compatible_dem() {
                ui.small("Point heights and DEM heights have unverified matching references; inspect them separately.");
            }
        }
    });
    model.target.radius_km = radius_km.clamp(0.1, 100.);
    if apply_radius {
        ui.ctx()
            .send_viewport_cmd(egui::ViewportCommand::CursorGrab(egui::CursorGrab::None));
        ui.ctx()
            .send_viewport_cmd(egui::ViewportCommand::CursorVisible(true));
        let region = Region {
            radius_m: model.target.radius_km * 1000.,
            ..session.region
        };
        let input = ContextInput::capture(model, region);
        let catalog = crate::lidar_catalog::db_path(
            Some(&model.settings_data_root),
            model.selected_root.as_deref(),
        );
        match session.resized(
            region,
            catalog,
            model.selected_root.clone(),
            input,
            ui.ctx(),
        ) {
            Ok(replacement) => {
                session = replacement;
                reload = false;
            }
            Err(error) => session.status = error,
        }
    }
    if !apply_radius && (reload || prior_source != session.selected_source) {
        let selected = session.sources.get(session.selected_source).cloned();
        session.start_points(selected, ui.ctx());
    }
    if reset {
        reset_camera(&mut session);
    }
    let (response, painter) = ui.allocate_painter(
        ui.available_size().max(egui::vec2(1., 1.)),
        egui::Sense::click_and_drag(),
    );
    let rect = response.rect;
    flight_input(&mut session, &response, ui.ctx(), fly);
    painter.rect_filled(rect, 0., egui::Color32::from_rgb(10, 18, 23));
    painter.add(egui_wgpu_callback(&session, rect, ui.ctx()));
    draw_boundary(&painter, rect, &session);
    if !session.loading() && session.points==0 && session.terrain.is_none() {
        painter.text(rect.center(),egui::Align2::CENTER_CENTER,
            "No Target data loaded here\nSurvey boundaries show published coverage; a readable point or DEM source is required.\nSee the source status above for details.",
            egui::FontId::proportional(16.),theme::text_muted());
    }
    if session.show_context
        && (!session.show_points || session.compatible_dem())
        && let Some(context) = &session.context
    {
        for (point, label) in &context.labels {
            if let Some(p) = session.camera.project(*point, rect)
                && rect.contains(p)
            {
                painter.circle_stroke(p, 4., egui::Stroke::new(1.0_f32, theme::text_muted()));
                painter.text(
                    p + egui::vec2(7., -7.),
                    egui::Align2::LEFT_BOTTOM,
                    label,
                    egui::FontId::proportional(11.),
                    theme::text_muted(),
                );
            }
        }
    }
    if session.camera.looking {
        painter.line_segment(
            [
                rect.center() - egui::vec2(5., 0.),
                rect.center() + egui::vec2(5., 0.),
            ],
            egui::Stroke::new(1.0_f32, theme::text_muted()),
        );
        painter.line_segment(
            [
                rect.center() - egui::vec2(0., 5.),
                rect.center() + egui::vec2(0., 5.),
            ],
            egui::Stroke::new(1.0_f32, theme::text_muted()),
        );
    }
    painter.text(rect.left_bottom()+egui::vec2(16.,-16.),egui::Align2::LEFT_BOTTOM,
        format!("CLICK TO FLY · MOUSE LOOK · WASD · SPACE / C UP / DOWN\nSHIFT BOOST · WHEEL SPEED · RMB RELEASE · HOME RESET · ESC LOCAL\nE {:.1} m  N {:.1} m  Z {:.2} {}",session.camera.eye[0],session.camera.eye[1],session.camera.eye[2],
            if session.show_points {"source units"} else {"m (DEM)"}),egui::FontId::monospace(11.),theme::text_muted());
    if session.loading() {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(100));
    }
    if exit {
        model.target.exit(ui.ctx());
    } else {
        model.target.session = Some(session);
    }
    rect
}
fn reset_camera(s: &mut Session) {
    let elevation = if s.show_points {
        s.clouds
            .first()
            .and_then(|b| b.points.first())
            .map(|p| p.position[2])
    } else {
        s.terrain.as_ref().map(|t| t.center_height())
    }
    .unwrap_or(0.);
    s.camera.reset(elevation);
}
fn egui_wgpu_callback(s: &Session, rect: egui::Rect, ctx: &egui::Context) -> egui::Shape {
    eframe::egui_wgpu::Callback::new_paint_callback(
        rect,
        target_pass::Draw {
            session: s.id,
            camera: s.camera.clone(),
            radius: s.region.radius_m as f32,
            clouds: s.clouds.clone(),
            terrain: s.terrain.clone(),
            context: s.context.clone(),
            points: s.show_points,
            surface: s.show_terrain,
            context_visible: s.show_context && (!s.show_points || s.compatible_dem()),
            class_mask: s.class_mask,
            color_mode: s.color_mode,
            point_size: s.point_size,
            rect,
            ctx: ctx.clone(),
        },
    )
    .into()
}
fn flight_input(s: &mut Session, response: &egui::Response, ctx: &egui::Context, fly: bool) {
    let looking = s.camera.looking;
    if fly || response.clicked_by(egui::PointerButton::Primary) {
        s.camera.looking = true;
    }
    let (mouse, dt, movement, fast, home, release, scroll) = ctx.input(|i| {
        let mut raw = egui::Vec2::ZERO;
        let mut has_raw = false;
        for event in &i.events {
            if let egui::Event::MouseMoved(delta) = event {
                raw += *delta;
                has_raw = true;
            }
        }
        let key = |key| if i.key_down(key) { 1. } else { 0. };
        (
            if has_raw { raw } else { i.pointer.delta() },
            i.stable_dt,
            [
                key(egui::Key::D) - key(egui::Key::A),
                key(egui::Key::W) - key(egui::Key::S),
                key(egui::Key::Space) - key(egui::Key::C),
            ],
            i.modifiers.shift,
            i.key_pressed(egui::Key::Home),
            !i.focused || i.pointer.button_pressed(egui::PointerButton::Secondary),
            i.raw_scroll_delta.y,
        )
    });
    if release {
        s.camera.looking = false;
    }
    if looking != s.camera.looking {
        ctx.send_viewport_cmd(egui::ViewportCommand::CursorGrab(if s.camera.looking {
            egui::CursorGrab::Locked
        } else {
            egui::CursorGrab::None
        }));
        ctx.send_viewport_cmd(egui::ViewportCommand::CursorVisible(!s.camera.looking));
    }
    if home && !ctx.wants_keyboard_input() {
        reset_camera(s);
    }
    if s.camera.looking {
        s.camera.speed = (s.camera.speed * (scroll * 0.002).exp()).clamp(1., 10_000.);
        // Ignore the initial capture-frame pointer warp.
        s.camera.step(
            movement,
            if looking { mouse } else { egui::Vec2::ZERO },
            dt,
            fast,
        );
        ctx.request_repaint();
    }
}
fn class_controls(ui: &mut egui::Ui, mask: &mut u32) {
    for (label, bits) in [
        ("Ground", 1 << 2),
        ("Vegetation", (1 << 3) | (1 << 4) | (1 << 5)),
        ("Buildings", 1 << 6),
        ("Water", 1 << 9),
        (
            "Other",
            !((1 << 2) | (1 << 3) | (1 << 4) | (1 << 5) | (1 << 6) | (1 << 9)),
        ),
    ] {
        let mut enabled = *mask & bits != 0;
        if ui.checkbox(&mut enabled, label).changed() {
            if enabled {
                *mask |= bits;
            } else {
                *mask &= !bits;
            }
        }
    }
}
fn draw_boundary(painter: &egui::Painter, rect: egui::Rect, s: &Session) {
    let height = if s.show_points {
        s.clouds
            .first()
            .and_then(|b| b.points.first())
            .map_or(0., |p| p.position[2])
    } else {
        s.terrain.as_ref().map_or(0., |t| t.center_height())
    };
    let color = egui::Color32::from_rgb(185, 66, 88);
    for i in 0..128 {
        let a = i as f32 * std::f32::consts::TAU / 128.;
        let b = (i + 1) as f32 * std::f32::consts::TAU / 128.;
        let r = s.region.radius_m as f32;
        if let (Some(a), Some(b)) = (
            s.camera.project([a.sin() * r, a.cos() * r, height], rect),
            s.camera.project([b.sin() * r, b.cos() * r, height], rect),
        ) && (rect.expand(200.).contains(a) || rect.expand(200.).contains(b))
        {
            painter.line_segment([a, b], egui::Stroke::new(1.0_f32, color));
        }
    }
}
