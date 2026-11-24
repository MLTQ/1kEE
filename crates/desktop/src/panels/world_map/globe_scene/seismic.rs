use super::{GlobeLayout, projection::project_visible_unit_vector};
use crate::model::seismic::{REPLAY_SPEED, SURFACE_SPEED_KM_S, Wave, unix_now};
use crate::model::{ActiveBody, AppModel, GeoPoint, GlobeViewState};
use crate::theme;
use egui::{Color32, Painter, Shape, Stroke};
use glam::Vec3;
use std::f32::consts::TAU;
use std::sync::OnceLock;
use std::time::Duration;

const SEGMENTS: usize = 192;
const FRONT_COLOR: Color32 = Color32::from_rgb(255, 176, 77);

pub(crate) fn controls(ui: &mut egui::Ui, model: &mut AppModel) {
    let available =
        can_draw(model) && !model.globe_view.local_mode && model.target.session.is_none();
    let checkbox = ui.add_enabled(
        available,
        egui::Checkbox::new(&mut model.seismic.enabled, "Seismic waves"),
    );
    if checkbox.changed() && !model.seismic.enabled {
        model.seismic.stop_replay();
        model.seismic.station_replay.stop();
    }
    checkbox.on_disabled_hover_text("Available on the live Earth globe");
    if !model.seismic.enabled || !available {
        return;
    }

    let time = ui.input(|input| input.time);
    ui.indent("seismic_waves", |ui| {
        ui.horizontal(|ui| {
            ui.hyperlink_to("EMSC", crate::seismic_stream::SOURCE_URL)
                .on_hover_text("Earthquake notifications from EMSC SeismicPortal, CC BY 4.0");
            ui.small(model.seismic.stream_status.label());
        });
        ui.small(format!("USGS feed · {}", model.usgs_stream_status));
        if super::station_replay::controls(ui, model, time) {
            return;
        }
        ui.small(format!("Modeled surface waves · {SURFACE_SPEED_KM_S} km/s"));
        if let Some(wave) = model.seismic.replay_wave(time) {
            ui.small(format!(
                "{}× replay · {} · {}",
                REPLAY_SPEED,
                wave.quake.provider.label(),
                wave.quake.title
            ));
            if ui.small_button("Return to live waves").clicked() {
                model.seismic.stop_replay();
            }
        } else if let Some(quake) = model
            .seismic
            .replay_candidate(model.selected_event_id.as_deref())
        {
            let label = if model.selected_event_id.as_deref() == Some(quake.id.as_str()) {
                "Replay modeled waves for selection"
            } else {
                "Replay latest modeled waves"
            };
            if ui.small_button(label).on_hover_text(&quake.title).clicked() {
                let quake = quake.clone();
                model.stop_event_follow();
                model.globe_view.stop_motion();
                model.globe_view.focus_on(quake.epicenter);
                model.globe_view.zoom = 1.0;
                model.seismic.start_replay(quake, time);
            }
        }
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
    time: f64,
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
    let replay_source = if let Some(wave) = model.seismic.replay_wave(time) {
        let source = wave.quake.provider.label();
        draw_wave(&painter, layout, view, wave);
        painter.ctx().request_repaint();
        Some(source)
    } else {
        let mut waves = model.seismic.live_waves(unix_now()).peekable();
        if waves.peek().is_none() {
            return;
        }
        for wave in waves {
            draw_wave(&painter, layout, view, wave);
        }
        painter
            .ctx()
            .request_repaint_after(Duration::from_millis(100));
        None
    };

    if !stereo {
        let label = if let Some(source) = replay_source {
            format!("{source} · Modeled waves · {REPLAY_SPEED}× replay")
        } else {
            "EMSC / USGS · Modeled waves · Live".to_owned()
        };
        let anchor = rect.left_top() + egui::vec2(23.0, 23.0);
        let text_color = theme::text_muted();
        let galley = painter.layout_no_wrap(label, egui::FontId::proportional(11.0), text_color);
        painter.rect_filled(
            egui::Rect::from_min_size(anchor, galley.size()).expand2(egui::vec2(6.0, 3.0)),
            4.0,
            theme::canvas_background(),
        );
        painter.galley(anchor, galley, text_color);
    }
}

fn draw_wave(painter: &Painter, layout: &GlobeLayout, view: &GlobeViewState, wave: Wave<'_>) {
    let (epicenter, tangent_east, tangent_north) = basis(wave.quake.epicenter);

    // Two dimmer echoes trail the modeled front to make its direction legible.
    for (lag, weight) in [(100.0, 0.24), (50.0, 0.48), (0.0, 1.0)] {
        let Some(radius) = wave.angular_radius(lag) else {
            continue;
        };
        let (sin_radius, cos_radius) = radius.sin_cos();
        let center = epicenter * cos_radius;
        let east = tangent_east * sin_radius;
        let north = tangent_north * sin_radius;
        let color = FRONT_COLOR.gamma_multiply(wave.opacity * weight);
        let mut path = Vec::new();
        for &(cos_bearing, sin_bearing) in bearings() {
            let point = center + east * cos_bearing + north * sin_bearing;
            if let Some(pos) = project_visible_unit_vector(layout, view, point) {
                path.push(pos);
            } else if !path.is_empty() {
                paint_arc(painter, std::mem::take(&mut path), color);
            }
        }
        paint_arc(painter, path, color);
    }

    if let Some(pos) = project_visible_unit_vector(layout, view, epicenter) {
        let color = FRONT_COLOR.gamma_multiply(wave.opacity);
        painter.circle_stroke(pos, 5.0, Stroke::new(1.0_f32, color.gamma_multiply(0.55)));
        painter.circle_filled(pos, 2.4, color);
    }
}

fn paint_arc(painter: &Painter, path: Vec<egui::Pos2>, color: Color32) {
    if path.len() >= 2 {
        painter.add(Shape::line(
            path.clone(),
            Stroke::new(5.0_f32, color.gamma_multiply(0.23)),
        ));
        painter.add(Shape::line(path, Stroke::new(1.5_f32, color)));
    }
}

// Use the globe projection's Cartesian axes so circles stay continuous across
// the antimeridian and can be centered at either pole.
fn basis(point: GeoPoint) -> (Vec3, Vec3, Vec3) {
    let epicenter = unit_vector(point);
    let (sin_lon, cos_lon) = point.lon.to_radians().sin_cos();
    let east = Vec3::new(-sin_lon, 0.0, cos_lon);
    (epicenter, east, east.cross(epicenter))
}

pub(super) fn unit_vector(point: GeoPoint) -> Vec3 {
    let (sin_lat, cos_lat) = point.lat.to_radians().sin_cos();
    let (sin_lon, cos_lon) = point.lon.to_radians().sin_cos();
    Vec3::new(cos_lat * cos_lon, sin_lat, cos_lat * sin_lon)
}

fn bearings() -> &'static [(f32, f32); SEGMENTS + 1] {
    static BEARINGS: OnceLock<[(f32, f32); SEGMENTS + 1]> = OnceLock::new();
    BEARINGS.get_or_init(|| {
        std::array::from_fn(|step| {
            let azimuth = (step % SEGMENTS) as f32 * TAU / SEGMENTS as f32;
            let (sin, cos) = azimuth.sin_cos();
            (cos, sin)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::EventSeverity;
    use crate::model::seismic::{Provider, Quake};

    #[test]
    fn rings_have_constant_geodesic_radius_at_poles_and_antimeridian() {
        let (sin, cos) = 0.8f32.sin_cos();
        for epicenter in [
            GeoPoint {
                lat: 90.0,
                lon: 42.0,
            },
            GeoPoint {
                lat: 20.0,
                lon: 179.0,
            },
        ] {
            let (center, east, north) = basis(epicenter);
            for &(x, y) in bearings() {
                let point = center * cos + (east * x + north * y) * sin;
                assert!((point.length() - 1.0).abs() < 1e-6);
                assert!((point.dot(center) - cos).abs() < 1e-6);
            }
        }
        assert_eq!(bearings().first(), bearings().last());
    }

    #[test]
    fn projection_masks_the_far_side_of_the_perspective_limb() {
        let view = GlobeViewState::from_focus(GeoPoint { lat: 0.0, lon: 0.0 });
        let layout = GlobeLayout {
            center: egui::Pos2::ZERO,
            radius: 100.0,
            focal_length: 2.0,
            camera_distance: 2.0,
        };
        let project =
            |lon| project_visible_unit_vector(&layout, &view, basis(GeoPoint { lat: 0.0, lon }).0);
        assert!(project(59.0).is_some());
        assert!(project(61.0).is_none());
        assert!(project(180.0).is_none());
    }

    #[test]
    fn wave_draws_expanding_paths_around_the_epicenter() {
        let ctx = egui::Context::default();
        let epicenter = GeoPoint { lat: 0.0, lon: 0.0 };
        let view = GlobeViewState::from_focus(epicenter);
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let layout = super::super::globe_layout(rect, &view);
        let quake = Quake {
            id: "example".into(),
            title: "Example quake".into(),
            epicenter,
            origin_ms: 0,
            severity: EventSeverity::Elevated,
            provider: Provider::Emsc,
            has_station_picks: false,
        };
        let radius_at = |age| {
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(rect),
                    ..Default::default()
                },
                |ctx| {
                    let painter = ctx.layer_painter(egui::LayerId::background());
                    draw_wave(
                        &painter,
                        &layout,
                        &view,
                        Wave {
                            quake: &quake,
                            age,
                            opacity: 1.0,
                        },
                    );
                },
            );
            output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    Shape::Path(path) => path
                        .points
                        .first()
                        .map(|point| point.distance(layout.center)),
                    _ => None,
                })
                .max_by(f32::total_cmp)
                .unwrap()
        };
        assert!(radius_at(120.0) > 1.9 * radius_at(60.0));
    }
}
