use super::{AGENCIES, Location, Transit, now};
use crate::model::{ActiveBody, AppModel, GeoPoint};

pub(crate) fn controls(ui: &mut egui::Ui, model: &mut AppModel) {
    ui.checkbox(&mut model.transit.enabled, "Live transit");
    if !model.transit.enabled {
        model.transit.selected = None;
        return;
    }
    let mut focus = None;
    let clock = now();
    ui.indent("transit_agencies", |ui| {
        ui.small("● Vehicle coordinate   □ Station or arrival");
        ui.checkbox(&mut model.transit.buses, "Include buses where available");
        for (i, agency) in AGENCIES.iter().enumerate() {
            let source = &mut model.transit.sources[i];
            ui.horizontal(|ui| {
                ui.checkbox(&mut source.enabled, agency.name);
                if ui.small_button("Locate").clicked() {
                    source.enabled = true;
                    focus = Some(agency.focus);
                }
            });
            if source.enabled {
                let visible = source
                    .reports
                    .iter()
                    .filter(|r| r.visible(clock, model.transit.buses))
                    .count();
                let status = if let Some(error) = &source.error {
                    format!("{visible} visible · {error}")
                } else if source.pending.is_some() || source.next_poll.is_none() {
                    format!("{visible} visible · refreshing")
                } else {
                    format!("{visible} recent reports")
                };
                ui.small(status);
                ui.hyperlink_to(egui::RichText::new(agency.credit).small(), agency.website);
            }
        }
    });
    if let Some(point) = focus {
        model.stop_event_follow();
        if model.active_body != ActiveBody::Earth {
            model.active_body = ActiveBody::Earth;
            model.map_theme = crate::theme::MapTheme::Topo;
            crate::theme::set_theme(ui.ctx(), model.map_theme);
        }
        model.globe_view.stop_motion();
        model.globe_view.focus_on(point);
        model.globe_view.local_zoom = 12.0;
        model.globe_view.local_mode = true;
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Marker {
    source: usize,
    index: usize,
    screen: egui::Pos2,
}

pub(crate) fn draw(
    painter: &egui::Painter,
    rect: egui::Rect,
    model: &AppModel,
    project: impl Fn(GeoPoint) -> Option<egui::Pos2>,
) -> Vec<Marker> {
    let transit = &model.transit;
    let mut markers = Vec::new();
    if !transit.enabled || model.active_body != ActiveBody::Earth {
        return markers;
    }
    let clock = now();
    for (source, state) in transit
        .sources
        .iter()
        .enumerate()
        .filter(|(_, s)| s.enabled)
    {
        for (index, r) in state
            .reports
            .iter()
            .enumerate()
            .filter(|(_, r)| r.visible(clock, transit.buses))
        {
            let Some(screen) = project(r.point).filter(|s| rect.contains(*s)) else {
                continue;
            };
            let radius = 4.0 * model.marker_scale();
            let outline = egui::Stroke::new(1.5_f32, egui::Color32::from_gray(225));
            let color = if clock.saturating_sub(r.time) > 90 {
                r.color.gamma_multiply(0.55)
            } else {
                r.color
            };
            match r.location {
                Location::Coordinate => {
                    painter.circle(screen, radius, color, outline);
                }
                Location::Station | Location::PredictedArrival => {
                    let box_rect =
                        egui::Rect::from_center_size(screen, egui::Vec2::splat(radius * 2.0));
                    painter.rect(box_rect, 1.0, color, outline, egui::StrokeKind::Middle);
                }
            }
            if transit
                .selected
                .as_ref()
                .is_some_and(|(s, id)| *s == source && id == &r.id)
            {
                painter.circle_stroke(
                    screen,
                    radius + 3.0,
                    egui::Stroke::new(2.0_f32, egui::Color32::WHITE),
                );
            }
            markers.push(Marker {
                source,
                index,
                screen,
            });
        }
    }
    markers
}

/// Hit test the same positions that were painted. Nearby trains at a shared
/// platform can be selected from the detail window after the first click.
pub(crate) fn pick(markers: &[Marker], cursor: egui::Pos2) -> Option<Marker> {
    markers
        .iter()
        .filter(|m| m.screen.distance(cursor) <= 10.0)
        .min_by(|a, b| {
            a.screen
                .distance_sq(cursor)
                .total_cmp(&b.screen.distance_sq(cursor))
        })
        .copied()
}

impl Transit {
    pub fn select(&mut self, marker: Marker) {
        self.selected = Some((
            marker.source,
            self.sources[marker.source].reports[marker.index].id.clone(),
        ));
    }
}

pub(crate) fn detail(ctx: &egui::Context, model: &mut AppModel) {
    let transit = &mut model.transit;
    if !transit.enabled || model.active_body != ActiveBody::Earth {
        return;
    }
    let Some((source, id)) = &transit.selected else {
        return;
    };
    let state = &transit.sources[*source];
    let clock = now();
    let Some(r) = state
        .reports
        .iter()
        .find(|r| &r.id == id && state.enabled && r.visible(clock, transit.buses))
    else {
        transit.selected = None;
        return;
    };
    let mut open = true;
    let mut switch = None;
    egui::Window::new("Transit report")
        .id("live_transit_detail".into())
        .open(&mut open)
        .show(ctx, |ui| {
            ui.strong(format!("{} · {}", r.route, r.label));
            ui.label(match r.location {
                Location::Coordinate => "Agency-reported coordinate",
                Location::Station => "Station reference",
                Location::PredictedArrival => "Predicted arrival at this station",
            });
            if !r.context.is_empty() {
                ui.label(&r.context);
            }
            let age = clock.saturating_sub(r.time);
            let stamp = if r.feed_time {
                "Feed timestamp"
            } else {
                "Report timestamp"
            };
            let freshness = if age > 90 { " · stale" } else { "" };
            ui.small(format!("{stamp} · {age}s ago{freshness}"));
            let agency = &AGENCIES[*source];
            ui.hyperlink_to(agency.credit, agency.website);
            let others: Vec<_> = state
                .reports
                .iter()
                .filter(|o| o.id != r.id && o.point == r.point && o.visible(clock, transit.buses))
                .collect();
            if !others.is_empty() {
                ui.collapsing(format!("{} other reports here", others.len()), |ui| {
                    for o in others {
                        if ui
                            .selectable_label(false, format!("{} · {}", o.route, o.label))
                            .clicked()
                        {
                            switch = Some((*source, o.id.clone()));
                        }
                    }
                });
            }
        });
    if !open || switch.is_some() {
        transit.selected = switch;
    }
}
