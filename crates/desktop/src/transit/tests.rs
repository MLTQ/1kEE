use super::*;
use serde_json::json;

#[test]
fn predictions_use_nearest_future_station_per_train_and_line() {
    let mut lookup = Lookup::default();
    for (key, lat) in [("near", 51.5), ("far", 51.6)] {
        lookup.stops.insert(
            key.into(),
            Station {
                point: point(lat, 0.1).unwrap(),
                name: key.into(),
            },
        );
    }
    let prediction = |line, stop, eta| {
        json!({
            "lineId":line, "lineName":line, "naptanId":stop, "vehicleId":"23",
            "timestamp":"2026-11-09T12:00:00Z", "expectedArrival":eta,
            "currentLocation":"Between stations"
        })
    };
    let clock = 1794225600;
    let data = json!([
        prediction("victoria", "far", "2026-11-09T12:06:00Z"),
        prediction("victoria", "far", "2026-11-09T11:59:55Z"),
        prediction("victoria", "near", "2026-11-09T12:01:00Z"),
        prediction("circle", "far", "2026-11-09T12:02:00Z"),
    ]);
    let reports = feeds::arrivals(&data, &lookup, clock).unwrap();
    assert_eq!(reports.len(), 2);
    let train = reports.iter().find(|r| r.route == "victoria").unwrap();
    assert_eq!(train.location, Location::PredictedArrival);
    assert_eq!(train.point, lookup.stops["near"].point);
    assert_eq!(train.time, clock);
    assert!(train.visible(clock + 60, false));
    assert!(!train.visible(clock + 61, false));
    assert!(!train.visible(clock + 301, false));
    assert!(!train.visible(clock - 1000, false));
}

#[test]
#[ignore = "uses public transit agency servers"]
fn live_sources() {
    let mut errors = Vec::new();
    for agency in AGENCIES {
        let update = feeds::refresh(agency, None);
        match update.reports {
            Ok(reports) => {
                let fresh = reports.iter().filter(|r| r.visible(now(), false)).count();
                eprintln!(
                    "{}: {} reports, {fresh} current rail reports",
                    agency.name,
                    reports.len()
                );
            }
            Err(error) => errors.push(format!("{}: {error}", agency.name)),
        }
    }
    assert!(errors.is_empty(), "{}", errors.join("\n"));
}

#[test]
fn map_marker_opens_its_report_and_obeys_layer_filters() {
    use crate::model::{ActiveBody, AppModel};
    let mut model = AppModel::new();
    model.transit.enabled = true;
    let report = |id: &str, bus: bool, time: u64| Report {
        id: id.into(),
        route: "1 Line".into(),
        label: "15".into(),
        context: "Stopped at Westlake".into(),
        location: Location::Coordinate,
        point: AGENCIES[0].focus,
        color: color("28813F"),
        time,
        feed_time: false,
        expires_at: None,
        bus,
    };
    let time = now();
    model.transit.sources[0].reports = vec![
        report("link", false, time),
        report("bus", true, time),
        report("old", false, time - 301),
    ];
    let ctx = egui::Context::default();
    let target = egui::pos2(100.0, 100.0);
    let frame = |model: &mut AppModel, events: Vec<egui::Event>| {
        let mut count = 0;
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(640.0, 480.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let (response, painter) =
                        ui.allocate_painter(ui.available_size(), egui::Sense::click_and_drag());
                    let markers = draw(&painter, response.rect, model, |_| Some(target));
                    count = markers.len();
                    if response.clicked()
                        && let Some(m) = response
                            .interact_pointer_pos()
                            .and_then(|pos| pick(&markers, pos))
                    {
                        model.transit.select(m);
                    }
                    detail(ctx, model);
                });
            },
        );
        count
    };
    let click = |pressed| egui::Event::PointerButton {
        pos: target,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    assert_eq!(frame(&mut model, vec![]), 1);
    frame(
        &mut model,
        vec![egui::Event::PointerMoved(target), click(true)],
    );
    frame(&mut model, vec![click(false)]);
    assert_eq!(model.transit.selected, Some((0, "link".into())));
    model.transit.buses = true;
    assert_eq!(frame(&mut model, vec![]), 2);
    model.active_body = ActiveBody::Moon;
    assert_eq!(frame(&mut model, vec![]), 0);
    model.active_body = ActiveBody::Earth;
    model.transit.enabled = false;
    assert_eq!(frame(&mut model, vec![]), 0);
}
