use super::{
    GlobeLayout,
    projection::project_visible_unit_vector,
    seismic::unit_vector,
    station_replay::{caption, phase_color},
};
use crate::model::{ActiveBody, AppModel};
use egui::{Color32, Painter, Rect, Vec2};

pub(super) fn draw(
    painter: &Painter,
    rect: Rect,
    layout: &GlobeLayout,
    model: &AppModel,
    stereo: bool,
) {
    if !model.replay_mode || model.active_body != ActiveBody::Earth {
        return;
    }
    let (Some(day), Some(clock)) = (&model.station_day, &model.replay_state) else {
        return;
    };
    let time_ms =
        ((clock.unix_at_elapsed(clock.wall_elapsed()) * 1000.0) as i64).min(day.to * 1000 - 1);
    // One second of screen time keeps brief arrivals visible in a compressed day.
    let flash_ms = 1000.0 * (day.to - day.from) as f64 / clock.wall_duration;
    let painter = painter.with_clip_rect(rect);
    let pointer = (!stereo)
        .then(|| painter.ctx().pointer_hover_pos())
        .flatten()
        .filter(|p| rect.contains(*p));
    let mut hovered = None;
    let mut inventory = egui::Mesh::default();
    let mut flashes = Vec::new();
    for (id, station) in &day.stations {
        let Some(point) = station.location(time_ms) else {
            continue;
        };
        let Some(pos) = project_visible_unit_vector(layout, &model.globe_view, unit_vector(point))
            .filter(|p| rect.contains(*p))
        else {
            continue;
        };
        inventory.add_colored_rect(
            Rect::from_center_size(pos, Vec2::splat(2.6)),
            Color32::from_rgb(112, 128, 143),
        );
        let arrival = station.last_arrival(time_ms);
        if let Some(hit) = arrival {
            let age = (time_ms - hit.pick.time_ms) as f64 / flash_ms;
            if age < 1.0 {
                let color = phase_color(hit.pick.family());
                flashes.push(egui::Shape::circle_filled(
                    pos,
                    2.5 + 2.0 * (1.0 - age as f32),
                    color.gamma_multiply(1.0 - 0.45 * age as f32),
                ));
            }
        }
        if let Some(d) = pointer.map(|p| p.distance_sq(pos)).filter(|d| *d <= 49.0)
            && hovered.as_ref().is_none_or(|(best, _, _)| d < *best)
        {
            hovered = Some((d, id, arrival));
        }
    }
    painter.add(inventory);
    painter.extend(flashes);
    if let (Some(pos), Some((_, id, arrival))) = (pointer, hovered) {
        let text = match arrival {
            Some(hit) => {
                crate::panels::seismogram::station_click(&painter, pos, id, Some((&hit.pick, &hit.event)));
                let time = chrono::DateTime::from_timestamp_millis(hit.pick.time_ms).unwrap();
                format!(
                    "{id} · {} at {} UTC\nUSGS {} · Right-click for seismogram",
                    hit.pick.phase,
                    time.format("%H:%M:%S%.3f"),
                    hit.event
                )
            }
            None => format!("{id} · No mapped arrival by this time"),
        };
        caption(
            &painter,
            egui::pos2(
                (pos.x + 12.).min(rect.right() - 240.),
                (pos.y + 12.).min(rect.bottom() - 38.),
            ),
            text,
        );
    }
}
