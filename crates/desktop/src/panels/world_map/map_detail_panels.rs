use crate::flight_tracks;
use crate::model::{AppModel, FlightCategory};
use crate::theme;

pub(super) fn draw_ship_detail_panel(ctx: &egui::Context, model: &mut AppModel) {
    let Some(mmsi) = model.selected_track_mmsi else {
        return;
    };

    // Clone the data we need so we don't hold a borrow on model.
    let track = model.tracks.iter().find(|t| t.mmsi == mmsi).cloned();

    let Some(track) = track else {
        // Vessel has left the cache — deselect.
        model.selected_track_mmsi = None;
        return;
    };

    let mut open = true;
    let ship_accent = egui::Color32::from_rgb(40, 210, 180);

    egui::Window::new("Vessel Detail")
        .id("ship_detail_panel".into())
        .open(&mut open)
        .default_size(egui::vec2(320.0, 360.0))
        .resizable(true)
        .collapsible(false)
        .frame(
            egui::Frame::window(&ctx.style())
                .fill(theme::window_fill())
                .stroke(egui::Stroke::new(1.0, ship_accent.gamma_multiply(0.5))),
        )
        .show(ctx, |ui| {
            ui.colored_label(ship_accent, track.ship_type_label());
            ui.heading(&track.name);
            ui.add_space(6.0);

            egui::Grid::new("vessel_fields")
                .num_columns(2)
                .spacing([12.0, 4.0])
                .show(ui, |ui| {
                    let mut row = |label: &str, value: &str| {
                        ui.colored_label(theme::text_muted(), label);
                        ui.label(value);
                        ui.end_row();
                    };

                    row("MMSI", &track.mmsi.to_string());

                    if let Some(imo) = track.imo {
                        row("IMO", &imo.to_string());
                    }
                    if let Some(cs) = &track.callsign {
                        row("Callsign", cs);
                    }
                    row(
                        "Position",
                        &format!("{:.4}°N  {:.4}°E", track.location.lat, track.location.lon),
                    );
                    if let Some(spd) = track.speed_knots {
                        row("Speed", &format!("{spd:.1} kn"));
                    }
                    if let Some(hdg) = track.heading_deg {
                        row("Heading", &format!("{hdg:.0}°"));
                    }
                    if let Some(dest) = &track.destination {
                        row("Destination", dest);
                    }
                    if let Some(eta) = &track.eta_str {
                        row("ETA", eta);
                    }
                    if let Some(d) = track.draught_m {
                        row("Draught", &format!("{d:.1} m"));
                    }
                });
        });

    if !open {
        model.selected_track_mmsi = None;
    }
}

pub(super) fn draw_flight_detail_panel(ctx: &egui::Context, model: &mut AppModel) {
    let Some(ref icao24) = model.selected_flight_icao24.clone() else {
        return;
    };

    // Deselect if the flight has left the active list.
    let flight = model.flights.iter().find(|f| f.icao24 == *icao24).cloned();
    let Some(flight) = flight else {
        model.selected_flight_icao24 = None;
        return;
    };

    let accent = match flight.category() {
        FlightCategory::Airline => theme::flight_airline_color(),
        FlightCategory::Cargo => theme::flight_cargo_color(),
        FlightCategory::Military => theme::flight_military_color(),
        FlightCategory::GA => theme::flight_ga_color(),
        FlightCategory::Unknown => theme::flight_unknown_color(),
    };
    let cat_label = match flight.category() {
        FlightCategory::Airline => "Scheduled Airline",
        FlightCategory::Cargo => "Cargo / Freight",
        FlightCategory::Military => "Military / Government",
        FlightCategory::GA => "General Aviation",
        FlightCategory::Unknown => "Unknown",
    };

    let mut open = true;
    egui::Window::new("Flight Detail")
        .id("flight_detail_panel".into())
        .open(&mut open)
        .default_size(egui::vec2(300.0, 340.0))
        .resizable(true)
        .collapsible(false)
        .frame(
            egui::Frame::window(&ctx.style())
                .fill(theme::window_fill())
                .stroke(egui::Stroke::new(1.0, accent.gamma_multiply(0.5))),
        )
        .show(ctx, |ui| {
            ui.colored_label(accent, cat_label);
            ui.heading(format!("✈ {}", flight.label()));
            ui.add_space(6.0);

            egui::Grid::new("flight_fields")
                .num_columns(2)
                .spacing([12.0, 4.0])
                .show(ui, |ui| {
                    let mut row = |label: &str, value: &str| {
                        ui.colored_label(theme::text_muted(), label);
                        ui.label(value);
                        ui.end_row();
                    };

                    row("ICAO24", &flight.icao24);
                    if let Some(cs) = &flight.callsign {
                        row("Callsign", cs);
                    }
                    if let Some(country) = &flight.origin_country {
                        row("Origin", country);
                    }
                    row(
                        "Position",
                        &format!("{:.4}°N  {:.4}°E", flight.location.lat, flight.location.lon),
                    );
                    row(
                        "Altitude",
                        &format!("{} {}", flight.altitude_label(), flight.trend_symbol()),
                    );
                    if let Some(spd) = flight.speed_knots {
                        row("Speed", &format!("{spd:.0} kn"));
                    }
                    if let Some(hdg) = flight.heading_deg {
                        row("Heading", &format!("{hdg:.0}°"));
                    }
                    if let Some(vr) = flight.vertical_rate_fpm {
                        row("Vert. rate", &format!("{vr:+.0} fpm"));
                    }
                });

            ui.add_space(8.0);
            ui.separator();
            ui.add_space(4.0);

            // ── Metadata section ───────────────────────────────────────────
            if flight_tracks::is_meta_loading(&flight.icao24) {
                ui.colored_label(theme::text_muted(), "⏳ Loading aircraft data…");
            } else if let Some(meta) = flight_tracks::get_metadata(&flight.icao24) {
                egui::Grid::new("meta_fields")
                    .num_columns(2)
                    .spacing([12.0, 4.0])
                    .show(ui, |ui| {
                        let mut row = |label: &str, value: &str| {
                            ui.colored_label(theme::text_muted(), label);
                            ui.label(value);
                            ui.end_row();
                        };
                        if let Some(reg) = &meta.registration {
                            row("Registration", reg);
                        }
                        if let Some(mfr) = &meta.manufacturer {
                            row("Manufacturer", mfr);
                        }
                        if let Some(mdl) = &meta.model {
                            row("Model", mdl);
                        }
                        if let Some(tc) = &meta.typecode {
                            row("Type", tc);
                        }
                        if let Some(op) = &meta.operator {
                            row("Operator", op);
                        }
                        if let Some(opc) = &meta.operator_callsign {
                            row("Op. callsign", opc);
                        }
                        if let Some(own) = &meta.owner {
                            row("Owner", own);
                        }
                    });
            } else {
                ui.colored_label(theme::text_muted(), "No aircraft record found.");
            }
        });

    if !open {
        model.selected_flight_icao24 = None;
    }
}

const LANDING_ACCENT: egui::Color32 = egui::Color32::from_rgb(255, 220, 50);

/// Ring the selected landing point, if it is among the projected markers.
/// Called by both scenes after they project landing points.
pub(super) fn ring_selected_landing(
    painter: &egui::Painter,
    markers: &[(usize, egui::Pos2)],
    model: &AppModel,
) {
    let Some(selected) = model.selected_landing_point else {
        return;
    };
    if let Some((_, pos)) = markers.iter().find(|(index, _)| *index == selected) {
        painter.circle_stroke(*pos, 9.0, egui::Stroke::new(1.5, LANDING_ACCENT));
        painter.circle_stroke(
            *pos,
            13.0,
            egui::Stroke::new(1.0, LANDING_ACCENT.gamma_multiply(0.35)),
        );
    }
}

/// Detail panel for a clicked cable landing point: the station, and every
/// cable that comes ashore there with its owners, length and service date.
pub(super) fn draw_landing_detail_panel(ctx: &egui::Context, model: &mut AppModel) {
    let Some(index) = model.selected_landing_point else {
        return;
    };
    // Hiding the layer (or cables entirely) closes the panel.
    let landing = crate::submarine_cables::landing_points_visible(model)
        .then(crate::submarine_cables::catalog)
        .flatten()
        .and_then(|catalog| Some((catalog, catalog.landings.get(index)?)));
    let Some((catalog, landing)) = landing else {
        model.selected_landing_point = None;
        return;
    };

    let planned = landing
        .cables
        .iter()
        .filter(|&&cable| catalog.cables[cable].is_planned)
        .count();
    let mut open = true;

    egui::Window::new("Landing Point")
        .id("landing_detail_panel".into())
        .open(&mut open)
        .default_size(egui::vec2(340.0, 380.0))
        .resizable(true)
        .collapsible(false)
        .frame(
            egui::Frame::window(&ctx.style())
                .fill(theme::window_fill())
                .stroke(egui::Stroke::new(1.0, LANDING_ACCENT.gamma_multiply(0.5))),
        )
        .show(ctx, |ui| {
            ui.colored_label(LANDING_ACCENT, "CABLE LANDING STATION");
            ui.heading(&landing.name);
            ui.add_space(6.0);

            egui::Grid::new("landing_fields")
                .num_columns(2)
                .spacing([12.0, 4.0])
                .show(ui, |ui| {
                    let mut row = |label: &str, value: &str| {
                        ui.colored_label(theme::text_muted(), label);
                        ui.label(value);
                        ui.end_row();
                    };
                    if let Some(country) = &landing.country {
                        row("Country", country);
                    }
                    row(
                        "Position",
                        &format!(
                            "{:.4}°N  {:.4}°E",
                            landing.location.lat, landing.location.lon
                        ),
                    );
                    let cables = match planned {
                        0 => landing.cables.len().to_string(),
                        planned => format!("{} ({planned} planned)", landing.cables.len()),
                    };
                    row("Cables", &cables);
                });

            if landing.cables.is_empty() {
                return;
            }
            ui.add_space(6.0);
            ui.separator();
            egui::ScrollArea::vertical()
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    for &cable_index in &landing.cables {
                        let cable = &catalog.cables[cable_index];
                        let [r, g, b, _] = cable.color;
                        ui.horizontal(|ui| {
                            let (dot, _) =
                                ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                            ui.painter().circle_filled(
                                dot.center(),
                                4.0,
                                egui::Color32::from_rgb(r, g, b),
                            );
                            match &cable.url {
                                Some(url) => {
                                    ui.hyperlink_to(egui::RichText::new(&cable.name).strong(), url);
                                }
                                None => {
                                    ui.strong(&cable.name);
                                }
                            }
                        });

                        let mut facts = Vec::new();
                        if cable.is_planned {
                            facts.push(match cable.rfs_year {
                                Some(year) => format!("Planned {year}"),
                                None => "Planned".to_owned(),
                            });
                        } else if let Some(rfs) = cable.rfs.as_ref() {
                            facts.push(format!("In service {rfs}"));
                        }
                        if let Some(length) = &cable.length {
                            facts.push(length.clone());
                        }
                        if !facts.is_empty() {
                            ui.small(
                                egui::RichText::new(facts.join(" · ")).color(theme::text_muted()),
                            );
                        }
                        if let Some(owners) = &cable.owners {
                            ui.small(egui::RichText::new(owners).color(theme::text_muted()));
                        }
                        ui.add_space(4.0);
                    }
                });
        });

    if !open {
        model.selected_landing_point = None;
    }
}
