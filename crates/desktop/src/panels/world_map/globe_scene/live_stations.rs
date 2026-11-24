use super::{
    GlobeLayout,
    projection::project_visible_unit_vector,
    seismic::unit_vector,
    station_replay::{caption, phase_color},
};
use crate::model::{ActiveBody, AppModel};
use crate::station_replay::live::{FLASH_MS, FRESH_MS};
use egui::{Color32, Painter, Rect, Stroke, Vec2};

const LISTED: Color32 = Color32::from_rgb(74, 79, 88);
const REPORTING: Color32 = Color32::from_rgb(116, 153, 170);
const MOTION: Color32 = Color32::from_rgb(255, 174, 68);

pub(crate) fn controls(ui: &mut egui::Ui, model: &mut AppModel) {
    let earth = model.active_body == ActiveBody::Earth
        && !model.globe_view.local_mode
        && !model.replay_mode
        && model.follow_recaps.active.is_none()
        && model.target.session.is_none();
    let live = &mut model.seismic.live_stations;
    ui.add_enabled(
        earth || live.enabled,
        egui::Checkbox::new(&mut live.enabled, "Live seismic stations"),
    )
    .on_hover_text(
        "Monitor real vertical seismometer and accelerometer waveforms on the live Earth globe.",
    );
    if live.enabled {
        ui.indent("live_station_status", |ui| {
            let count = format!("{} reporting · {} listed", live.reporting, live.stations.len());
            ui.small(count).on_hover_text(
                "Blue-gray stations sent a sample in the past two minutes. Gray stations have no recent waveform.",
            );
            ui.small(if live.stream_status.is_empty() {
                "Connecting to EarthScope"
            } else {
                &live.stream_status
            });
            ui.small("Amber · elevated motion").on_hover_text(
                "A sudden rise above this channel’s recent background. Local disturbances can trigger it too.",
            );
            ui.small("Published arrivals · P cyan · S violet · other green")
                .on_hover_text("USGS phase picks light up when first reported. Hover to see the measured arrival time and event ID.");
            if !live.inventory_status.is_empty() {
                ui.small(&live.inventory_status);
            }
            if !live.catalog_status.is_empty() {
                ui.small(&live.catalog_status);
            }
        });
    }
}

pub(super) fn draw(
    painter: &Painter,
    rect: Rect,
    layout: &GlobeLayout,
    model: &AppModel,
    stereo: bool,
) {
    let layer = &model.seismic.live_stations;
    if !layer.enabled
        || model.active_body != ActiveBody::Earth
        || model.globe_view.local_mode
        || model.target.session.is_some()
        || model.replay_mode
        || model.follow_recaps.active.is_some()
        || super::station_replay::is_active(model)
        || model
            .seismic
            .replay_wave(painter.ctx().input(|i| i.time))
            .is_some()
    {
        return;
    }
    let now = chrono::Utc::now().timestamp_millis();
    let painter = painter.with_clip_rect(rect);
    let pointer = (!stereo)
        .then(|| painter.ctx().pointer_hover_pos())
        .flatten()
        .filter(|p| rect.contains(*p));
    let mut hovered = None;
    let mut mesh = egui::Mesh::default();
    let mut flashes = Vec::new();
    for (id, station) in &layer.stations {
        let Some(pos) = station
            .location(now)
            .and_then(|p| project_visible_unit_vector(layout, &model.globe_view, unit_vector(p)))
            .filter(|p| rect.contains(*p))
        else {
            continue;
        };
        let state = layer.activity.get(id);
        let fresh = state.is_some_and(|a| a.sample_ms >= now - FRESH_MS);
        mesh.add_colored_rect(
            Rect::from_center_size(pos, Vec2::splat(if fresh { 3.0 } else { 2.0 })),
            if fresh { REPORTING } else { LISTED },
        );
        let official = state
            .and_then(|a| a.report.as_ref())
            .filter(|r| r.received_ms >= now - FLASH_MS);
        let motion = state
            .and_then(|a| a.motion.as_ref())
            .filter(|m| m.received_ms >= now - FLASH_MS);
        if let Some((color, onset)) = official
            .map(|r| (phase_color(r.pick.family()), r.received_ms))
            .or_else(|| motion.map(|m| (MOTION, m.received_ms)))
        {
            let fade = (1.0 - (now - onset) as f32 / FLASH_MS as f32).clamp(0., 1.);
            flashes.push(egui::Shape::circle_filled(
                pos,
                2.0 + 2.5 * fade,
                color.gamma_multiply(0.35 + 0.65 * fade),
            ));
            flashes.push(egui::Shape::circle_stroke(
                pos,
                4.5 + (1.0 - fade) * 7.0,
                Stroke::new(1.0_f32, color.gamma_multiply(fade)),
            ));
        }
        if let Some(distance) = pointer.map(|p| p.distance_sq(pos)).filter(|&d| d <= 49.)
            && hovered.as_ref().is_none_or(|(best, _, _)| distance < *best)
        {
            hovered = Some((distance, id, state));
        }
    }
    painter.add(mesh);
    if !flashes.is_empty() {
        painter.extend(flashes);
        painter
            .ctx()
            .request_repaint_after(std::time::Duration::from_millis(100));
    }
    if let (Some(pos), Some((_, id, state))) = (pointer, hovered) {
        let mut lines = vec![id.to_string()];
        if let Some(state) = state {
            if state.sample_ms > 0 {
                lines.push(format!("Last sample {} UTC", utc(state.sample_ms)));
            }
            if let Some(m) = &state.motion {
                lines.push(format!(
                    "Motion {} UTC · {:.1}× background",
                    utc(m.observed_ms),
                    m.ratio
                ));
            }
            if let Some(r) = &state.report {
                lines.push(format!(
                    "{} at {} UTC · USGS {}",
                    r.pick.phase,
                    utc(r.pick.time_ms),
                    r.event
                ));
            }
        } else {
            lines.push("No recent station data".into());
        }
        let anchor = egui::pos2(
            (pos.x + 12.).min(rect.right() - 270.),
            (pos.y + 12.).min(rect.bottom() - 17. * lines.len() as f32),
        );
        caption(&painter, anchor, lines.join("\n"));
    }
}

fn utc(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .unwrap()
        .format("%H:%M:%S")
        .to_string()
}
