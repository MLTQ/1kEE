use super::catalog::{Agency, Feed};
use super::{
    CachedLookup, Location, Lookup, Report, Result, Station, Update, color, gtfs, now, point,
};
use chrono::DateTime;
use reqwest::blocking::{Client, RequestBuilder};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::sync::Arc;
use std::time::{Duration, Instant};

const NYC: &str = "https://demo.transiter.dev/systems/us-ny-subway";
const LONDON: &str = "https://api.tfl.gov.uk/Line/bakerloo,central,circle,district,hammersmith-city,jubilee,metropolitan,northern,piccadilly,victoria,waterloo-city/Arrivals";
const FINLAND: &str = "https://rata.digitraffic.fi/api/v1/train-locations/latest";

fn bytes(request: RequestBuilder, limit: u64) -> Result<Vec<u8>> {
    let response = request.send()?.error_for_status()?;
    let gzip = response
        .headers()
        .get("Content-Encoding")
        .is_some_and(|v| v.as_bytes().eq_ignore_ascii_case(b"gzip"));
    let reader: Box<dyn Read> = if gzip {
        Box::new(flate2::read::GzDecoder::new(response))
    } else {
        Box::new(response)
    };
    let mut buf = Vec::new();
    reader.take(limit + 1).read_to_end(&mut buf)?;
    if buf.len() as u64 > limit {
        return Err("Feed exceeds download limit".into());
    }
    Ok(buf)
}

fn json(request: RequestBuilder) -> Result<Value> {
    Ok(serde_json::from_slice(&bytes(request, 32 << 20)?)?)
}

fn str_at<'a>(row: &'a Value, field: &str) -> &'a str {
    row[field].as_str().unwrap_or_default().trim()
}

fn timestamp(value: &Value) -> Option<u64> {
    DateTime::parse_from_rfc3339(value.as_str()?)
        .ok()?
        .timestamp()
        .try_into()
        .ok()
}

fn station(row: &Value, lat: &str, lon: &str, name: &str) -> Option<Station> {
    Some(Station {
        point: point(row[lat].as_f64()?, row[lon].as_f64()?)?,
        name: row[name].as_str()?.to_owned(),
    })
}

fn ny_pages(client: &Client, resource: &str, mut visit: impl FnMut(&Value)) -> Result<()> {
    let mut cursor = String::new();
    let mut seen = HashSet::new();
    loop {
        let mut request = client
            .get(format!("{NYC}/{resource}"))
            .query(&[("limit", "100")]);
        if resource == "stops" {
            request = request.query(&[
                ("skip_stop_times", "true"),
                ("skip_service_maps", "true"),
                ("skip_alerts", "true"),
                ("skip_transfers", "true"),
            ]);
        }
        if !cursor.is_empty() {
            request = request.query(&[("first_id", &cursor)]);
        }
        let page = json(request)?;
        for row in page[resource].as_array().ok_or("Missing Transiter page")? {
            visit(row);
        }
        cursor = str_at(&page, "nextId").into();
        if cursor.is_empty() {
            return Ok(());
        }
        if !seen.insert(cursor.clone()) || seen.len() > 64 {
            return Err("Invalid Transiter pagination".into());
        }
    }
}

fn load(client: &Client, feed: &Feed) -> Result<Lookup> {
    let mut lookup = Lookup::default();
    match feed {
        Feed::Gtfs { schedule, .. } => {
            return gtfs::schedule(bytes(client.get(*schedule), 64 << 20)?);
        }
        Feed::NewYork => {
            ny_pages(client, "stops", |s| {
                if let Some(stop) = station(s, "latitude", "longitude", "name") {
                    lookup.stops.insert(str_at(s, "id").into(), stop);
                }
            })?;
        }
        Feed::London => {
            let data = json(client.get("https://api.tfl.gov.uk/StopPoint/Mode/tube"))?;
            for s in data["stopPoints"]
                .as_array()
                .ok_or("Missing TfL stations")?
            {
                if let Some(stop) = station(s, "lat", "lon", "commonName") {
                    lookup.stops.insert(str_at(s, "naptanId").into(), stop);
                }
            }
        }
        Feed::Finland => {}
    }
    Ok(lookup)
}

fn ny_vehicle(v: &Value, lookup: &Lookup) -> Option<Report> {
    let stop = lookup.stops.get(str_at(&v["stop"], "id"))?;
    let route = &v["trip"]["route"];
    let status = match str_at(v, "currentStatus") {
        "STOPPED_AT" => "Stopped at",
        "INCOMING_AT" => "Approaching",
        "IN_TRANSIT_TO" => "In transit to",
        _ => "Reported station",
    };
    let id = v["id"].as_str()?;
    Some(Report {
        id: id.into(),
        label: id.into(),
        route: route["id"].as_str()?.into(),
        context: format!("{status} {}", stop.name),
        location: Location::Station,
        point: stop.point,
        color: color(str_at(route, "color")),
        time: str_at(v, "updatedAt").parse().ok()?,
        feed_time: false,
        expires_at: None,
        bus: false,
    })
}

fn tube_color(line: &str) -> egui::Color32 {
    color(match line {
        "bakerloo" => "B36305",
        "central" => "E32017",
        "circle" => "FFD300",
        "district" => "00782A",
        "hammersmith-city" => "F3A9BB",
        "jubilee" => "A0A5A9",
        "metropolitan" => "9B0056",
        "northern" => "000000",
        "piccadilly" => "003688",
        "victoria" => "0098D4",
        "waterloo-city" => "95CDBA",
        _ => "",
    })
}

pub(super) fn arrivals(data: &Value, lookup: &Lookup, clock: u64) -> Result<Vec<Report>> {
    // TfL publishes one prediction per train per downstream platform. Anchor a
    // train's earliest still-future prediction at the *arrival station*.
    let mut nearest: HashMap<String, (u64, Report)> = HashMap::new();
    for v in data.as_array().ok_or("Expected TfL arrivals")? {
        let label = str_at(v, "vehicleId");
        let line = str_at(v, "lineId");
        let id = format!("{line}/{label}");
        let Some(eta) = timestamp(&v["expectedArrival"]) else {
            continue;
        };
        let Some(time) = timestamp(&v["timestamp"]) else {
            continue;
        };
        let Some(stop) = lookup.stops.get(str_at(v, "naptanId")) else {
            continue;
        };
        if label.is_empty()
            || label == "000"
            || line.is_empty()
            || eta < clock
            || clock.saturating_sub(time) > 300
            || nearest.get(&id).is_some_and(|(prior, _)| *prior <= eta)
        {
            continue;
        }
        let mut context = format!(
            "Expected at {} · {} UTC",
            stop.name,
            crate::event_store::unix_to_datetime_str(eta as i64)
        );
        let reported = str_at(v, "currentLocation");
        if !reported.is_empty() {
            context.push_str(&format!("\nTfL location · {reported}"));
        }
        nearest.insert(
            id.clone(),
            (
                eta,
                Report {
                    id,
                    label: label.into(),
                    route: str_at(v, "lineName").into(),
                    context,
                    location: Location::PredictedArrival,
                    point: stop.point,
                    color: tube_color(line),
                    time,
                    bus: false,
                    feed_time: false,
                    expires_at: Some(eta),
                },
            ),
        );
    }
    Ok(nearest.into_values().map(|(_, report)| report).collect())
}

fn finnish_train(v: &Value) -> Option<Report> {
    let coords = v["location"]["coordinates"].as_array()?;
    let p = point(coords.get(1)?.as_f64()?, coords.first()?.as_f64()?)?;
    let train = v["trainNumber"].as_u64()?.to_string();
    let description = if v["isGpsLocation"].as_bool() == Some(true) {
        "GPS position"
    } else {
        "Agency location"
    };
    let speed = v["speed"]
        .as_f64()
        .map(|s| format!(" · {s:.0} km/h"))
        .unwrap_or_default();
    Some(Report {
        id: format!("{}/{}", str_at(v, "departureDate"), train),
        label: train,
        route: "Train".into(),
        context: format!("{description}{speed}"),
        point: p,
        color: color("49BC72"),
        location: Location::Coordinate,
        time: timestamp(&v["timestamp"])?,
        bus: false,
        feed_time: false,
        expires_at: None,
    })
}

fn fetch(client: &Client, feed: &Feed, lookup: &Lookup) -> Result<Vec<Report>> {
    match feed {
        Feed::Gtfs { vehicles, .. } => {
            gtfs::vehicles(&bytes(client.get(*vehicles), 16 << 20)?, lookup)
        }
        Feed::NewYork => {
            let mut reports = Vec::new();
            ny_pages(client, "vehicles", |v| {
                reports.extend(ny_vehicle(v, lookup))
            })?;
            Ok(reports)
        }
        Feed::London => arrivals(&json(client.get(LONDON))?, lookup, now()),
        Feed::Finland => {
            let data = json(client.get(FINLAND).header("Digitraffic-User", "1kEE/0.1"))?;
            Ok(data
                .as_array()
                .ok_or("Expected Digitraffic locations")?
                .iter()
                .filter_map(finnish_train)
                .collect())
        }
    }
}

pub(super) fn refresh(agency: &Agency, mut cached: Option<CachedLookup>) -> Update {
    let reports = (|| -> Result<Vec<Report>> {
        let client = Client::builder()
            .timeout(Duration::from_secs(25))
            .user_agent("1kEE/0.1")
            .default_headers({
                let mut h = reqwest::header::HeaderMap::new();
                h.insert(
                    reqwest::header::ACCEPT_ENCODING,
                    reqwest::header::HeaderValue::from_static("gzip"),
                );
                h
            })
            .build()?;
        let lookup = match &cached {
            Some((loaded, lookup)) if loaded.elapsed() < Duration::from_secs(86400) => {
                Arc::clone(lookup)
            }
            _ => {
                let lookup = Arc::new(load(&client, &agency.feed)?);
                cached = Some((Instant::now(), Arc::clone(&lookup)));
                lookup
            }
        };
        let mut reports = fetch(&client, &agency.feed, &lookup)?;
        reports.sort_by(|a, b| a.id.cmp(&b.id).then(b.time.cmp(&a.time)));
        reports.dedup_by(|a, b| a.id == b.id);
        Ok(reports)
    })();
    Update {
        lookup: cached,
        reports,
    }
}
