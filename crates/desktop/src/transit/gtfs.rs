use super::{Location, Lookup, Report, Result, Station, color, point};
use protobuf::Message;
use serde::Deserialize;
use std::collections::HashMap;
use std::io::{Cursor, Read};
use zip::ZipArchive;

#[allow(warnings)]
mod wire {
    include!(concat!(env!("OUT_DIR"), "/mod.rs"));
}
use wire::gtfs_realtime::{
    FeedHeader_Incrementality, FeedMessage, VehiclePosition_VehicleStopStatus,
};

#[derive(Deserialize)]
pub(super) struct Route {
    route_id: String,
    route_short_name: String,
    route_long_name: String,
    route_type: u16,
    #[serde(default)]
    route_color: String,
}
#[derive(Deserialize)]
pub(super) struct Trip {
    route_id: String,
    trip_id: String,
    #[serde(default)]
    trip_headsign: String,
}
#[derive(Deserialize)]
struct Stop {
    stop_id: String,
    stop_name: String,
    stop_lat: Option<f64>,
    stop_lon: Option<f64>,
}

fn rows<T: serde::de::DeserializeOwned>(
    zip: &mut ZipArchive<Cursor<Vec<u8>>>,
    file: &str,
    mut add: impl FnMut(T),
) -> Result<()> {
    let entry = zip.by_name(file)?;
    if entry.size() > 64 * 1024 * 1024 {
        return Err(format!("{file} exceeds 64 MiB").into());
    }
    let mut csv = csv::Reader::from_reader(entry.take(64 * 1024 * 1024));
    for row in csv.deserialize() {
        add(row?);
    }
    Ok(())
}

#[derive(Deserialize)]
struct Translation {
    language: String,
    field_name: String,
    #[serde(default)]
    field_value: String,
    translation: String,
}

pub(super) fn schedule(bytes: Vec<u8>) -> Result<Lookup> {
    let mut zip = ZipArchive::new(Cursor::new(bytes))?;
    let mut english: HashMap<String, HashMap<String, String>> = HashMap::new();
    if zip.file_names().any(|name| name == "translations.txt") {
        rows(&mut zip, "translations.txt", |t: Translation| {
            if t.language == "en" && !t.field_value.is_empty() && !t.translation.is_empty() {
                english
                    .entry(t.field_name)
                    .or_default()
                    .insert(t.field_value, t.translation);
            }
        })?;
    }
    let translate = |field: &str, value: &mut String| {
        if let Some(name) = english.get(field).and_then(|names| names.get(value)) {
            value.clone_from(name);
        }
    };
    let mut lookup = Lookup::default();
    rows(&mut zip, "routes.txt", |mut r: Route| {
        translate("route_short_name", &mut r.route_short_name);
        translate("route_long_name", &mut r.route_long_name);
        lookup.routes.insert(r.route_id.clone(), r);
    })?;
    rows(&mut zip, "trips.txt", |mut t: Trip| {
        translate("trip_headsign", &mut t.trip_headsign);
        lookup.trips.insert(t.trip_id.clone(), t);
    })?;
    rows(&mut zip, "stops.txt", |mut s: Stop| {
        translate("stop_name", &mut s.stop_name);
        if let Some(p) = s
            .stop_lat
            .zip(s.stop_lon)
            .and_then(|(lat, lon)| point(lat, lon))
        {
            lookup.stops.insert(
                s.stop_id,
                Station {
                    name: s.stop_name,
                    point: p,
                },
            );
        }
    })?;
    Ok(lookup)
}

pub(super) fn vehicles(bytes: &[u8], lookup: &Lookup) -> Result<Vec<Report>> {
    let feed = FeedMessage::parse_from_bytes(bytes)?;
    let header = feed.get_header();
    if header.get_incrementality() != FeedHeader_Incrementality::FULL_DATASET {
        return Err("Expected a full vehicle snapshot".into());
    }
    // Some rail-only feeds omit the trip/route descriptor on an otherwise
    // valid vehicle fix. Mixed-mode agencies require positive mode identification.
    let rail_type = |kind: u16| matches!(kind, 0..=2 | 5 | 7 | 12);
    let rail_only =
        !lookup.routes.is_empty() && lookup.routes.values().all(|r| rail_type(r.route_type));
    Ok(feed
        .get_entity()
        .iter()
        .filter_map(|e| {
            if e.get_is_deleted() || !e.has_vehicle() {
                return None;
            }
            let v = e.get_vehicle();
            let trip = v.get_trip();
            let scheduled = lookup.trips.get(trip.get_trip_id());
            let route = lookup
                .routes
                .get(trip.get_route_id())
                .or_else(|| scheduled.and_then(|t| lookup.routes.get(&t.route_id)));
            let bus = match route.map(|r| r.route_type) {
                Some(3) => true,
                Some(kind) if rail_type(kind) => false,
                None if rail_only => false,
                _ => return None,
            };
            let stop = lookup.stops.get(v.get_stop_id());
            let pos = v.get_position();
            let (location, p) = if pos.has_latitude() && pos.has_longitude() {
                (
                    Location::Coordinate,
                    point(pos.get_latitude() as f64, pos.get_longitude() as f64)?,
                )
            } else {
                (Location::Station, stop?.point)
            };
            let id = [v.get_vehicle().get_id(), trip.get_trip_id(), e.get_id()]
                .into_iter()
                .find(|id| !id.is_empty())?;
            let label = [v.get_vehicle().get_label(), id]
                .into_iter()
                .find(|s| !s.is_empty())?;
            let status = match v.get_current_status() {
                VehiclePosition_VehicleStopStatus::STOPPED_AT => "Stopped at",
                VehiclePosition_VehicleStopStatus::INCOMING_AT => "Approaching",
                VehiclePosition_VehicleStopStatus::IN_TRANSIT_TO => "In transit to",
            };
            let mut context = stop
                .map(|s| format!("{status} {}", s.name))
                .unwrap_or_default();
            if let Some(headsign) = scheduled
                .map(|t| t.trip_headsign.trim())
                .filter(|s| !s.is_empty())
            {
                if !context.is_empty() {
                    context.push('\n');
                }
                context.push_str(&format!("Destination · {headsign}"));
            }
            Some(Report {
                id: id.into(),
                label: label.into(),
                route: route
                    .and_then(|r| {
                        [&r.route_short_name, &r.route_long_name, &r.route_id]
                            .into_iter()
                            .find(|name| !name.is_empty())
                    })
                    .cloned()
                    .unwrap_or_else(|| "Train".into()),
                context,
                location,
                point: p,
                color: color(route.map_or("", |r| &r.route_color)),
                bus,
                time: if v.has_timestamp() {
                    v.get_timestamp()
                } else {
                    header.get_timestamp()
                },
                feed_time: !v.has_timestamp(),
                expires_at: None,
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wire::gtfs_realtime::{FeedEntity, VehiclePosition};

    #[test]
    fn roundtrip_distinguishes_coordinates_station_reference_and_missing_location() {
        let mut lookup = Lookup::default();
        lookup.stops.insert(
            "A".into(),
            Station {
                name: "Central".into(),
                point: point(51.0, 1.0).unwrap(),
            },
        );
        lookup.routes.insert(
            "R".into(),
            Route {
                route_id: "R".into(),
                route_type: 1,
                route_color: "366339".into(),
                route_short_name: "R".into(),
                route_long_name: String::new(),
            },
        );
        lookup.trips.insert(
            "trip".into(),
            Trip {
                route_id: "R".into(),
                trip_id: "trip".into(),
                trip_headsign: "Park".into(),
            },
        );
        let mut feed = FeedMessage::new();
        feed.mut_header().set_gtfs_realtime_version("2.0".into());
        feed.mut_header().set_timestamp(300);
        for i in 0..3 {
            let mut entity = FeedEntity::new();
            entity.set_id(i.to_string());
            let mut vehicle = VehiclePosition::new();
            vehicle.mut_trip().set_trip_id("trip".into());
            if i != 2 {
                vehicle.set_stop_id("A".into());
            }
            if i == 0 {
                vehicle.mut_position().set_latitude(51.25);
                vehicle.mut_position().set_longitude(1.125);
                vehicle.set_timestamp(290);
            }
            entity.set_vehicle(vehicle);
            feed.mut_entity().push(entity);
        }
        let reports = vehicles(&feed.write_to_bytes().unwrap(), &lookup).unwrap();
        assert_eq!(reports.len(), 2);
        assert_eq!(reports[0].point, point(51.25, 1.125).unwrap());
        assert_eq!(reports[0].location, Location::Coordinate);
        assert!(!reports[0].feed_time);
        assert_eq!(reports[0].time, 290);
        assert_eq!(reports[1].point, lookup.stops["A"].point);
        assert_eq!(reports[1].location, Location::Station);
        assert!(reports[1].context.contains("Central"));
        assert!(reports[1].feed_time);
        assert_eq!(reports[1].time, 300);
        assert!(vehicles(b"invalid", &lookup).is_err());
    }
}
