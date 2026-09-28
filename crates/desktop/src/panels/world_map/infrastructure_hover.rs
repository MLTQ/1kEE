//! One descriptive hover card for infrastructure, shared by both map scenes.
use egui::{Context, Pos2};
use std::cell::RefCell;
use tile_archive::{pipelines::Info, platforms::Platform};

pub(super) const RADIUS: f32 = 7.0;
#[derive(Clone)]
enum Detail {
    Pipeline(Info),
    Platform(Platform),
}
impl Detail {
    fn id(&self) -> (&str, &str) {
        match self {
            Self::Pipeline(p) => (&p.source, &p.source_id),
            Self::Platform(p) => (&p.source, &p.source_id),
        }
    }
    fn name(&self) -> &str {
        match self {
            Self::Pipeline(p) => {
                if p.name.is_empty() {
                    "Unnamed pipeline"
                } else {
                    &p.name
                }
            }
            Self::Platform(p) => {
                if p.name.is_empty() {
                    "Offshore installation"
                } else {
                    &p.name
                }
            }
        }
    }
    fn priority(&self) -> u8 {
        match self {
            Self::Platform(_) => 0,
            Self::Pipeline(_) => 1,
        }
    }
}
struct Candidate {
    detail: Detail,
    distance: f32,
}
#[derive(Default)]
struct Frame {
    pointer: Option<Pos2>,
    hits: Vec<Candidate>,
}
thread_local! { static FRAME: RefCell<Frame> = RefCell::new(Frame::default()); }

/// Called before scene painting. No hover can leak across frames/layer changes.
pub(super) fn begin(pointer: Option<Pos2>) {
    FRAME.with(|f| {
        let mut f = f.borrow_mut();
        f.pointer = pointer;
        f.hits.clear();
    });
}
pub(super) fn pointer() -> Option<Pos2> {
    FRAME.with(|f| f.borrow().pointer)
}
fn offer(detail: impl FnOnce() -> Detail, source: &str, id: &str, priority: u8, distance: f32) {
    if !distance.is_finite() || distance > RADIUS {
        return;
    }
    FRAME.with(|frame| {
        let mut frame = frame.borrow_mut();
        if frame.pointer.is_none() {
            return;
        }
        if let Some(hit) = frame
            .hits
            .iter_mut()
            .find(|h| h.detail.priority() == priority && h.detail.id() == (source, id))
        {
            hit.distance = hit.distance.min(distance);
        } else {
            frame.hits.push(Candidate {
                detail: detail(),
                distance,
            });
        }
    });
}
pub(super) fn pipeline(info: &Info, distance: f32) {
    offer(
        || Detail::Pipeline(info.clone()),
        &info.source,
        &info.source_id,
        1,
        distance,
    );
}
pub(super) fn platform(info: &Platform, distance: f32) {
    offer(
        || Detail::Platform(info.clone()),
        &info.source,
        &info.source_id,
        0,
        distance,
    );
}

pub(super) fn line_distance(pointer: Pos2, a: Pos2, b: Pos2) -> f32 {
    if !egui::Rect::from_two_pos(a, b)
        .expand(RADIUS)
        .contains(pointer)
    {
        return f32::INFINITY;
    }
    let d = b - a;
    let t = ((pointer - a).dot(d) / d.length_sq().max(1e-12)).clamp(0.0, 1.0);
    pointer.distance(a + d * t)
}
fn source_name(source: &str) -> &str {
    match source {
        "gem-gas" | "gem-oil" => "Global Energy Monitor",
        "bsee" => "BSEE",
        "emodnet" => "EMODnet / Cogea",
        "osm" => "OpenStreetMap",
        s if s.starts_with("eia-") => "EIA / DOE",
        s => s,
    }
}
fn rows(detail: &Detail) -> Vec<(&'static str, String)> {
    let mut rows = Vec::new();
    match detail {
        Detail::Pipeline(p) => {
            rows.push(("Type", format!("{} pipeline", p.product)));
            rows.push((
                "Status",
                if p.status.is_empty() {
                    "Unknown".into()
                } else {
                    p.status.clone()
                },
            ));
            for (label, value) in [("Operator", &p.operator), ("Owner", &p.owner)] {
                if !value.is_empty() {
                    rows.push((label, value.clone()));
                }
            }
            if !p.accuracy.is_empty() {
                rows.push(("Route accuracy", p.accuracy.clone()));
            }
        }
        Detail::Platform(p) => {
            for (label, value) in [
                ("Type", &p.kind),
                ("Country", &p.country),
                ("Status", &p.status),
                ("Operator", &p.operator),
                ("Production", &p.product),
                ("Function", &p.function),
                ("Installed / valid from", &p.installed),
                ("Removed / valid to", &p.removed),
            ] {
                if !value.is_empty() {
                    rows.push((label, value.clone()));
                }
            }
            if let Some(depth) = p.water_depth_m {
                rows.push(("Water depth", format!("{depth:.0} m")));
            }
            rows.push(("Position", format!("{:.5}, {:.5}", p.lat, p.lon)));
            rows.push((
                "Location data",
                "Static inventory; not a live rig position".into(),
            ));
        }
    }
    let (source, id) = detail.id();
    rows.push(("Source", source_name(source).into()));
    rows.push(("Source ID", id.into()));
    rows
}

/// Returns true when infrastructure owns this frame's tooltip.
pub(super) fn show(ctx: &Context) -> bool {
    FRAME.with(|frame| {
        let mut frame = frame.borrow_mut();
        let Some(pointer) = frame.pointer else {
            return false;
        };
        frame.hits.sort_by(|a, b| {
            a.detail
                .priority()
                .cmp(&b.detail.priority())
                .then(a.distance.total_cmp(&b.distance))
                .then(a.detail.id().cmp(&b.detail.id()))
        });
        let Some(first) = frame.hits.first() else {
            return false;
        };
        egui::Area::new("infrastructure-hover".into())
            .order(egui::Order::Tooltip)
            .interactable(false)
            .constrain_to(ctx.screen_rect())
            .fixed_pos(pointer + egui::vec2(14.0, 14.0))
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_width(350.0);
                    ui.strong(first.detail.name());
                    for (label, value) in rows(&first.detail) {
                        ui.label(format!("{label}: {value}"));
                    }
                    if frame.hits.len() > 1 {
                        ui.separator();
                        ui.small(format!(
                            "Also under cursor: {} other records",
                            frame.hits.len() - 1
                        ));
                        for hit in frame.hits.iter().skip(1).take(4) {
                            let (source, id) = hit.detail.id();
                            ui.small(format!(
                                "{} · {} {id}",
                                hit.detail.name(),
                                source_name(source)
                            ));
                        }
                    }
                });
            });
        true
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn repeated_fragments_deduplicate_and_frame_reset_clears_hits() {
        let info = Info {
            source: "gem-gas".into(),
            source_id: "P1".into(),
            name: "Example".into(),
            operator: "Operator".into(),
            owner: "Owner".into(),
            product: "gas".into(),
            status: "unknown".into(),
            accuracy: "Approximate".into(),
            historical: false,
            planned: false,
        };
        begin(Some(Pos2::ZERO));
        pipeline(&info, 5.0);
        pipeline(&info, 2.0);
        FRAME.with(|f| {
            let f = f.borrow();
            assert_eq!(f.hits.len(), 1);
            assert_eq!(f.hits[0].distance, 2.0);
        });
        let data = rows(&Detail::Pipeline(info));
        assert!(data.contains(&("Owner", "Owner".into())));
        assert!(data.contains(&("Operator", "Operator".into())));
        assert!(data.contains(&("Route accuracy", "Approximate".into())));
        begin(None);
        FRAME.with(|f| assert!(f.borrow().hits.is_empty()));
    }
    #[test]
    fn platform_hover_card_renders_reported_fields_and_overlap_summary() {
        let p: Platform = serde_json::from_value(serde_json::json!({
            "source":"bsee","source_id":"77/1","name":"Test platform","operator":"Reported operator",
            "country":"United States","product":"Oil","kind":"Platform","function":"Production",
            "status":"Operating status unknown","installed":"1988-01-01","removed":"","water_depth_m":41.0,
            "historical":false,"planned":false,"support":false,"lat":28.0,"lon":-90.0
        })).unwrap();
        let info = Info {
            source: "gem-gas".into(),
            source_id: "P1".into(),
            name: "Nearby pipeline".into(),
            operator: String::new(),
            owner: String::new(),
            product: "gas".into(),
            status: "unknown".into(),
            accuracy: "Approximate".into(),
            historical: false,
            planned: false,
        };
        let ctx = Context::default();
        let mut text = String::new();
        for frame in 0..3 {
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        Pos2::ZERO,
                        egui::vec2(800.0, 600.0),
                    )),
                    time: Some(frame as f64 * 0.1),
                    ..Default::default()
                },
                |ctx| {
                    begin(Some(egui::pos2(780.0, 580.0)));
                    pipeline(&info, 0.0);
                    platform(&p, 3.0);
                    assert!(show(ctx));
                },
            );
            for shape in output.shapes {
                if let egui::Shape::Text(t) = shape.shape {
                    text.push_str(t.galley.text());
                    text.push('\n');
                }
            }
        }
        for expected in [
            "Test platform",
            "Reported operator",
            "Operating status unknown",
            "41 m",
            "Static inventory",
            "Source: BSEE",
            "Source ID: 77/1",
            "Nearby pipeline",
            "Also under cursor: 1",
        ] {
            assert!(text.contains(expected), "missing {expected}: {text}");
        }
        begin(None);
    }
}
