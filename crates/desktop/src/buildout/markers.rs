use super::{Source, view::Kind};
use crate::model::{ActiveBody, GeoPoint};
use egui::{Color32, Pos2};

#[derive(Clone, Copy)]
pub struct Marker {
    kind: Kind,
    index: usize,
    pos: Pos2,
}

pub(super) fn stage_color(stage: &str) -> Color32 {
    match stage {
        "operational" => Color32::from_rgb(81, 205, 156),
        "construction" => Color32::from_rgb(244, 176, 80),
        "planned" => Color32::from_rgb(113, 179, 248),
        "paused" => Color32::from_rgb(213, 159, 204),
        _ => Color32::LIGHT_GRAY,
    }
}

pub fn draw(
    painter: &egui::Painter,
    source: &Source,
    body: ActiveBody,
    project: impl Fn(GeoPoint) -> Option<Pos2>,
) -> Vec<Marker> {
    let mut markers = Vec::new();
    if body != ActiveBody::Earth || !source.enabled {
        return markers;
    }
    let Some(snapshot) = &source.snapshot else {
        return markers;
    };
    let mut mark = |kind, index, lat, lon, color, approximate| {
        if let Some(pos) =
            project(GeoPoint { lat, lon }).filter(|p| painter.clip_rect().contains(*p))
        {
            if kind == Kind::Rule {
                painter.rect_stroke(
                    egui::Rect::from_center_size(pos, egui::vec2(9.0, 9.0)),
                    0,
                    (1.5, color),
                    egui::StrokeKind::Inside,
                );
            } else if approximate {
                painter.circle_stroke(pos, 3.5, (1.5, color));
            } else {
                painter.circle_filled(pos, 3.5, color);
            }
            markers.push(Marker { kind, index, pos });
        }
    };
    for (i, site) in snapshot.catalog.campuses.iter().enumerate() {
        mark(
            Kind::Campus,
            i,
            site.lat,
            site.lng,
            stage_color(&site.status),
            site.precision != "campus",
        );
    }
    for (i, rule) in snapshot.catalog.rules.iter().enumerate() {
        if let (Some(lat), Some(lon)) = (rule.lat, rule.lng) {
            mark(
                Kind::Rule,
                i,
                lat,
                lon,
                Color32::from_rgb(223, 143, 236),
                false,
            );
        }
    }
    markers
}

pub fn pick(source: &mut Source, markers: &[Marker], pointer: Pos2) {
    let Some(hit) = markers
        .iter()
        .filter(|m| m.pos.distance(pointer) <= 8.0)
        .min_by(|a, b| {
            a.pos
                .distance_sq(pointer)
                .total_cmp(&b.pos.distance_sq(pointer))
        })
    else {
        return;
    };
    let Some(snapshot) = &source.snapshot else {
        return;
    };
    let id = match hit.kind {
        Kind::Campus => &snapshot.catalog.campuses[hit.index].id,
        Kind::Rule => &snapshot.catalog.rules[hit.index].id,
        Kind::News => return,
    };
    source.browser.select(hit.kind, id);
}
