use crate::model::{AppModel, FlightCategory};
use crate::theme;
use super::globe_scene;

pub(super) fn draw_event_hover_tooltip(
    ctx: &egui::Context,
    model: &AppModel,
    scene: &globe_scene::GlobeScene,
    hover_pos: Option<egui::Pos2>,
) {
    let Some(pointer) = hover_pos else {
        return;
    };

    let Some((event_id, marker_pos)) = scene
        .event_markers
        .iter()
        .find(|(_, marker)| marker.distance(pointer) <= 12.0)
    else {
        return;
    };

    let Some(event) = model.events.iter().find(|event| event.id == *event_id) else {
        return;
    };

    egui::Area::new("event_hover_tooltip".into())
        .fixed_pos(*marker_pos + egui::vec2(14.0, -8.0))
        .interactable(false)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(theme::panel_fill(238))
                .stroke(egui::Stroke::new(1.0, theme::panel_stroke()))
                .corner_radius(8.0)
                .inner_margin(egui::Margin::same(8))
                .show(ui, |ui| {
                    ui.colored_label(event.severity.color(), event.severity.label());
                    ui.strong(event.title.as_str());
                    ui.small(event.location_name.as_str());
                });
        });
}

pub(super) fn draw_ship_hover_tooltip(
    ctx: &egui::Context,
    model: &AppModel,
    scene: &globe_scene::GlobeScene,
    hover_pos: Option<egui::Pos2>,
) {
    let Some(pointer) = hover_pos else { return };

    // Don't show hover tooltip if a ship is already selected (detail panel visible).
    if model.selected_track_mmsi.is_some() { return; }

    let Some(&(mmsi, marker_pos)) = scene
        .ship_markers
        .iter()
        .find(|(_, marker)| marker.distance(pointer) <= 12.0)
    else {
        return;
    };

    let Some(track) = model.tracks.iter().find(|t| t.mmsi == mmsi) else {
        return;
    };

    egui::Area::new("ship_hover_tooltip".into())
        .fixed_pos(marker_pos + egui::vec2(14.0, -8.0))
        .interactable(false)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(theme::panel_fill(238))
                .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(40, 210, 180).gamma_multiply(0.5)))
                .corner_radius(8.0)
                .inner_margin(egui::Margin::same(8))
                .show(ui, |ui| {
                    ui.colored_label(
                        egui::Color32::from_rgb(40, 210, 180),
                        track.ship_type_label(),
                    );
                    ui.strong(&track.name);
                    ui.small(format!("MMSI {}", track.mmsi));
                    if let Some(spd) = track.speed_knots {
                        ui.small(format!("{:.1} kn", spd));
                    }
                });
        });
}

pub(super) fn draw_flight_hover_tooltip(
    ctx: &egui::Context,
    model: &AppModel,
    scene: &globe_scene::GlobeScene,
    hover_pos: Option<egui::Pos2>,
) {
    // Suppress hover tooltip while a flight detail panel is open.
    if model.selected_flight_icao24.is_some() { return; }

    let Some(pointer) = hover_pos else { return };
    let Some((icao24, marker_pos)) = scene
        .flight_markers
        .iter()
        .find(|(_, pos)| pos.distance(pointer) <= 12.0)
    else {
        return;
    };
    let Some(flight) = model.flights.iter().find(|f| &f.icao24 == icao24) else {
        return;
    };

    let col = match flight.category() {
        FlightCategory::Airline  => theme::flight_airline_color(),
        FlightCategory::Cargo    => theme::flight_cargo_color(),
        FlightCategory::Military => theme::flight_military_color(),
        FlightCategory::GA       => theme::flight_ga_color(),
        FlightCategory::Unknown  => theme::flight_unknown_color(),
    };
    let cat_label = match flight.category() {
        FlightCategory::Airline  => "Airline",
        FlightCategory::Cargo    => "Cargo",
        FlightCategory::Military => "Military",
        FlightCategory::GA       => "General Aviation",
        FlightCategory::Unknown  => "",
    };

    egui::Area::new("flight_hover_tooltip".into())
        .fixed_pos(*marker_pos + egui::vec2(14.0, -8.0))
        .interactable(false)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(theme::panel_fill(238))
                .stroke(egui::Stroke::new(1.0, col.gamma_multiply(0.5)))
                .corner_radius(8.0)
                .inner_margin(egui::Margin::same(8))
                .show(ui, |ui| {
                    ui.colored_label(col, format!("✈ {}", flight.label()));
                    if !cat_label.is_empty() {
                        ui.small(egui::RichText::new(cat_label).color(col.gamma_multiply(0.75)));
                    }
                    ui.small(format!(
                        "{} {} {}",
                        flight.altitude_label(),
                        flight.trend_symbol(),
                        flight.origin_country.as_deref().unwrap_or(""),
                    ));
                    if let Some(spd) = flight.speed_knots {
                        ui.small(format!("{:.0} kn", spd));
                    }
                    ui.small(egui::RichText::new("click for details")
                        .color(theme::text_muted()));
                });
        });
}
