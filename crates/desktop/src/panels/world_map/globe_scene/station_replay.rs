use super::{GlobeLayout, projection::project_visible_unit_vector, seismic::unit_vector};
use crate::model::{AppModel, GlobeViewState};
use crate::station_replay::{Phase, Playback};
use crate::theme;
use egui::{Color32, Painter, Pos2, Rect, Stroke};

const P_COLOR: Color32 = Color32::from_rgb(84, 225, 255);
const S_COLOR: Color32 = Color32::from_rgb(239, 143, 246);
const OTHER_COLOR: Color32 = Color32::from_rgb(175, 220, 150);
const CATALOG_COLOR: Color32 = Color32::from_rgb(255, 185, 91);
const FIT_COLOR: Color32 = Color32::WHITE;

pub(super) fn is_active(model: &AppModel) -> bool {
    model.seismic.enabled && model.seismic.station_replay.playback.is_some()
}

pub(super) fn controls(ui: &mut egui::Ui, model: &mut AppModel, time: f64) -> bool {
    if let Some(playback) = &mut model.seismic.station_replay.playback {
        let recording = &playback.recording;
        ui.small(&recording.title);
        ui.small(format!(
            "Located stations: {} of {}",
            recording.stations.len(),
            recording.total_stations
        ));
        match &recording.fit {
            Ok(fit) => {
                ui.small(format!(
                    "Regional fit: {} stations · {:.2} s RMS · {:.1} km from catalog",
                    fit.stations, fit.rms_s, fit.offset_km
                ));
                ui.small(format!(
                    "Origin time {:+.2} s vs. catalog · depth fixed at {:.1} km",
                    fit.time_offset_s, recording.origin.depth_km
                ));
            }
            Err(reason) => {
                ui.small(*reason);
            }
        }
        egui::ComboBox::from_id_salt("station_replay_speed")
            .selected_text(format!("{}×", playback.speed))
            .show_ui(ui, |ui| {
                for speed in [1., 2., 5., 10., 20., 60.] {
                    ui.selectable_value(&mut playback.speed, speed, format!("{speed}×"));
                }
            });
        if ui
            .add(
                egui::Slider::new(&mut playback.position, playback.start..=playback.end)
                    .suffix(" s")
                    .max_decimals(2),
            )
            .on_hover_text("Seconds since the catalog origin time")
            .changed()
        {
            playback.playing = false;
        }
        let action = if playback.playing {
            "Pause"
        } else if playback.position >= playback.end {
            "Replay"
        } else {
            "Play"
        };
        let exit = ui
            .horizontal(|ui| {
                if ui.small_button(action).clicked() {
                    playback.toggle();
                }
                ui.small_button("Exit replay").clicked()
            })
            .inner;
        if exit {
            model.seismic.station_replay.stop();
        }
        return true;
    }

    if model.seismic.station_replay.is_loading() {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.small("Loading station arrivals");
        });
        if ui.small_button("Cancel").clicked() {
            model.seismic.station_replay.stop();
        }
        return true;
    }

    let selected_id = model.selected_event_id.as_deref();
    let mut request = None;
    if let Some(quake) = model.seismic.station_replay_candidate(selected_id) {
        let label = if Some(quake.id.as_str()) == selected_id {
            "Replay recorded arrivals"
        } else if quake.has_station_picks {
            "Replay latest with station data"
        } else {
            "Replay latest USGS arrivals"
        };
        if ui.small_button(label).on_hover_text(&quake.title).clicked() {
            request = Some(quake.id.clone());
        }
    }
    ui.collapsing("Replay by USGS event ID", |ui| {
        let id = &mut model.seismic.station_replay.id_entry;
        let input = ui.add(
            egui::TextEdit::singleline(id)
                .hint_text("e.g. ci38457511")
                .char_limit(48),
        );
        let submitted = input.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
        if ui.small_button("Load recorded arrivals").clicked() || submitted {
            request = Some(id.clone());
        }
    });
    if let Some(error) = &model.seismic.station_replay.status {
        ui.small(error);
    }
    if let Some(id) = request {
        model.seismic.stop_replay();
        if let Some((point, zoom)) = model.seismic.station_replay.open(&id, ui.ctx(), time) {
            model.focus_station_replay(point, zoom);
        }
        return true;
    }
    false
}

pub(super) fn draw(
    painter: &Painter,
    rect: Rect,
    layout: &GlobeLayout,
    view: &GlobeViewState,
    playback: &Playback,
    stereo: bool,
) {
    let recording = &playback.recording;
    let project = |point| project_visible_unit_vector(layout, view, unit_vector(point));
    let pointer = (!stereo)
        .then(|| painter.ctx().pointer_hover_pos())
        .flatten()
        .filter(|pointer| rect.contains(*pointer));
    let mut hovered = None;
    for station in &recording.stations {
        let Some(pos) =
            project(station.location.point).filter(|pos| rect.expand(12.0).contains(*pos))
        else {
            continue;
        };
        let recent = playback.current_pick(station);
        if let Some((pick, age)) = recent {
            let color = match pick.family() {
                Phase::P => P_COLOR,
                Phase::S => S_COLOR,
                Phase::Other => OTHER_COLOR,
            };
            let flash = (1.0 - age / playback.speed.max(1.0)).clamp(0.0, 1.0) as f32;
            painter.circle_filled(
                pos,
                2.5 + flash * 2.0,
                color.gamma_multiply(0.65 + flash * 0.35),
            );
            if flash > 0.0 {
                painter.circle_stroke(
                    pos,
                    5.0 + (1.0 - flash) * 7.0,
                    Stroke::new(1.4_f32, color.gamma_multiply(flash)),
                );
            }
        } else {
            painter.circle_stroke(
                pos,
                2.5,
                Stroke::new(1.0_f32, theme::text_muted().gamma_multiply(0.8)),
            );
        }
        if let Some(distance) = pointer
            .map(|pointer| pointer.distance(pos))
            .filter(|distance| *distance <= 8.0)
            && hovered.as_ref().is_none_or(|(best, _, _)| distance < *best)
        {
            hovered = Some((distance, station, recent));
        }
    }

    if let Some(pos) = project(recording.origin.point) {
        painter.circle_stroke(pos, 7.0, Stroke::new(1.7_f32, CATALOG_COLOR));
        painter.circle_filled(pos, 2.0, CATALOG_COLOR);
    }
    if let Ok(fit) = &recording.fit
        && let Some(pos) = project(fit.point)
    {
        painter.line_segment(
            [pos - egui::vec2(4.0, 0.0), pos + egui::vec2(4.0, 0.0)],
            Stroke::new(1.4_f32, FIT_COLOR),
        );
        painter.line_segment(
            [pos - egui::vec2(0.0, 4.0), pos + egui::vec2(0.0, 4.0)],
            Stroke::new(1.4_f32, FIT_COLOR),
        );
    }

    if playback.playing {
        painter.ctx().request_repaint();
    }
    if stereo {
        return;
    }

    let fit_label = if recording.fit.is_ok() {
        " · + Regional fit"
    } else {
        ""
    };
    let label = format!(
        "Recorded station arrivals · {:+.2} s · {}×\nP cyan · S violet · ○ Catalog epicenter{fit_label}",
        playback.position, playback.speed
    );
    caption(painter, rect.left_top() + egui::vec2(23.0, 23.0), label);

    if let Some((_, station, recent)) = hovered
        && let Some(pointer) = pointer
    {
        let label = match recent {
            Some((pick, _)) => format!(
                "{} · {} · {:+.3} s",
                station.station,
                pick.phase,
                recording.offset_s(pick.time_ms)
            ),
            None => format!("{} · Awaiting arrival", station.station),
        };
        let pos = egui::pos2(
            (pointer.x + 12.0).min(rect.right() - 175.0),
            (pointer.y + 12.0).min(rect.bottom() - 22.0),
        );
        caption(painter, pos, label);
    }
}

fn caption(painter: &Painter, anchor: Pos2, label: String) {
    let ink = theme::text_muted();
    let galley = painter.layout_no_wrap(label, egui::FontId::proportional(11.0), ink);
    painter.rect_filled(
        Rect::from_min_size(anchor, galley.size()).expand2(egui::vec2(6.0, 3.0)),
        4.0,
        theme::canvas_background(),
    );
    painter.galley(anchor, galley, ink);
}
