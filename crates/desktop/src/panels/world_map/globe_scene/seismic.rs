use super::{GlobeLayout, projection::project_visible_unit_vector};
use crate::model::{ActiveBody, AppModel, GeoPoint};
use egui::{Color32, Painter};
use glam::Vec3;

const EPICENTER_COLOR: Color32 = Color32::from_rgb(255, 176, 77);

pub(crate) fn controls(ui: &mut egui::Ui, model: &mut AppModel) {
    let available = can_draw(model);
    let checkbox = ui.add_enabled(
        available,
        egui::Checkbox::new(&mut model.seismic.enabled, "Earthquake epicenters"),
    );
    if checkbox.changed() && !model.seismic.enabled {
        model.seismic.station_replay.stop();
    }
    checkbox.on_disabled_hover_text("Available on the live Earth globe");
    if !model.seismic.enabled || !available {
        return;
    }

    let time = ui.input(|input| input.time);
    ui.indent("earthquake_epicenters", |ui| {
        ui.horizontal(|ui| {
            ui.hyperlink_to("EMSC", crate::seismic_stream::SOURCE_URL)
                .on_hover_text("Earthquake notifications from EMSC SeismicPortal, CC BY 4.0");
            ui.small(model.seismic.stream_status.label());
        });
        ui.small(format!("USGS feed · {}", model.usgs_stream_status));
        super::station_replay::controls(ui, model, time);
    });
}

fn can_draw(model: &AppModel) -> bool {
    model.active_body == ActiveBody::Earth
        && !model.replay_mode
        && model.follow_recaps.active.is_none()
}

pub(super) fn draw(
    painter: &Painter,
    rect: egui::Rect,
    layout: &GlobeLayout,
    model: &AppModel,
    stereo: bool,
) {
    if !model.seismic.enabled || !can_draw(model) {
        return;
    }

    let painter = painter.with_clip_rect(rect);
    let view = &model.globe_view;
    if let Some(playback) = &model.seismic.station_replay.playback {
        super::station_replay::draw(&painter, rect, layout, view, playback, stereo);
        return;
    }
    let now_ms = chrono::Utc::now().timestamp_millis();
    for quake in model.seismic.recent_epicenters(now_ms) {
        if let Some(pos) = project_visible_unit_vector(layout, view, unit_vector(quake.epicenter)) {
            painter.circle_filled(pos, 2.4, EPICENTER_COLOR);
        }
    }
}

pub(super) fn unit_vector(point: GeoPoint) -> Vec3 {
    let (sin_lat, cos_lat) = point.lat.to_radians().sin_cos();
    let (sin_lon, cos_lon) = point.lon.to_radians().sin_cos();
    Vec3::new(cos_lat * cos_lon, sin_lat, cos_lat * sin_lon)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{EventSeverity, GlobeViewState, seismic::Quake};

    #[test]
    fn projection_masks_the_far_side_of_the_perspective_limb() {
        let view = GlobeViewState::from_focus(GeoPoint { lat: 0.0, lon: 0.0 });
        let layout = GlobeLayout {
            center: egui::Pos2::ZERO,
            radius: 100.0,
            focal_length: 2.0,
            camera_distance: 2.0,
        };
        let project = |lon| {
            project_visible_unit_vector(&layout, &view, unit_vector(GeoPoint { lat: 0.0, lon }))
        };
        assert!(project(59.0).is_some());
        assert!(project(61.0).is_none());
        assert!(project(180.0).is_none());
    }

    #[test]
    fn reported_epicenter_stays_a_single_fixed_dot_as_the_quake_ages() {
        let ctx = egui::Context::default();
        let mut model = AppModel::new();
        let epicenter = GeoPoint { lat: 0.0, lon: 0.0 };
        model.globe_view = GlobeViewState::from_focus(epicenter);
        model.seismic.enabled = true;
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let layout = super::super::globe_layout(rect, &model.globe_view);
        for age_ms in [60_000, 120_000] {
            model.seismic.replace_quakes(vec![Quake {
                id: "example".into(),
                title: "Example quake".into(),
                epicenter,
                origin_ms: chrono::Utc::now().timestamp_millis() - age_ms,
                severity: EventSeverity::Elevated,
                has_station_picks: false,
            }]);
            let frame = ctx.run(
                egui::RawInput {
                    screen_rect: Some(rect),
                    ..Default::default()
                },
                |ctx| {
                    draw(
                        &ctx.layer_painter(egui::LayerId::background()),
                        rect,
                        &layout,
                        &model,
                        false,
                    );
                },
            );
            let shapes: Vec<_> = frame
                .shapes
                .iter()
                .filter(|s| !matches!(s.shape, egui::Shape::Noop))
                .collect();
            assert!(matches!(&shapes[..], [s] if matches!(&s.shape,
                egui::Shape::Circle(dot) if dot.center.distance(layout.center) < 0.01 && dot.radius == 2.4)));
        }
    }
}
