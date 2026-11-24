use super::{Observation, Origin, Pick, Recording, Station, StationId, locate};
use crate::model::GeoPoint;
use chrono::{DateTime, SecondsFormat, Utc};
use reqwest::blocking::{Client, RequestBuilder};
use reqwest::header::CONTENT_TYPE;
use roxmltree::{Document, Node};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const USGS_QUERY: &str = "https://earthquake.usgs.gov/fdsnws/event/1/query";
const USGS_DETAILS: &str = "https://earthquake.usgs.gov/earthquakes/feed/v1.0/detail";
const STATIONS: &str = "https://service.earthscope.org/fdsnws/station/1/query";
const EIDA_STATIONS: &str = "https://www.orfeus-eu.org/fdsnws/station/1/query";
const RESPONSE_LIMIT: u64 = 12 * 1024 * 1024;
const STATION_BATCH: usize = 250;

pub(super) fn load(id: &str, cancelled: &AtomicBool) -> Result<Recording, String> {
    let check_active = || {
        if cancelled.load(Ordering::Relaxed) {
            Err("Station replay cancelled.")
        } else {
            Ok(())
        }
    };
    check_active()?;
    let client = Client::builder()
        .timeout(Duration::from_secs(12))
        .build()
        .map_err(|error| format!("Could not start station replay: {error}"))?;
    let body = read(
        client.get(format!("{USGS_DETAILS}/{id}.geojson")),
        "USGS event",
    );
    let body = match body {
        Ok(body) => body,
        Err(_) => {
            check_active()?;
            read(
                client
                    .get(USGS_QUERY)
                    .query(&[("eventid", id), ("format", "geojson")]),
                "USGS event",
            )?
        }
    };
    let json: Value = serde_json::from_str(&body).map_err(|_| "Invalid USGS event data.")?;
    let (title, origin, products) = event(&json)?;
    let mut picks = None;
    let mut phase_error = "USGS has no usable phase arrivals for this event.".to_owned();
    for url in products.into_iter().take(3) {
        check_active()?;
        match read(client.get(url), "USGS phase data").and_then(|xml| phase_picks(&xml)) {
            Ok(parsed) if !parsed.is_empty() => {
                picks = Some(parsed);
                break;
            }
            Ok(_) => {}
            Err(error) => phase_error = error,
        }
    }
    let picks = picks.ok_or(phase_error)?;
    let time = DateTime::<Utc>::from_timestamp_millis(origin.time_ms)
        .ok_or("Invalid USGS origin time.")?;
    let mut stations = HashMap::new();
    let mut metadata_error = None;
    for service in [STATIONS, EIDA_STATIONS] {
        let missing: Vec<_> = picks
            .keys()
            .filter(|key| !stations.contains_key(*key))
            .cloned()
            .collect();
        for batch in missing.chunks(STATION_BATCH) {
            check_active()?;
            match fetch_stations(&client, service, batch, time) {
                Ok(result) => stations.extend(result),
                Err(error) => metadata_error = Some(error),
            }
        }
    }
    assemble(title, origin, picks, &stations).map_err(|error| metadata_error.unwrap_or(error))
}

fn event(json: &Value) -> Result<(String, Origin, Vec<reqwest::Url>), String> {
    let feature = json
        .get("features")
        .and_then(Value::as_array)
        .and_then(|features| features.first())
        .unwrap_or(json);
    let props = &feature["properties"];
    if props.get("type").and_then(Value::as_str) != Some("earthquake") {
        return Err("The USGS event is unavailable or is not classified as an earthquake.".into());
    }
    let title = props["title"]
        .as_str()
        .unwrap_or("USGS earthquake")
        .to_owned();
    let geometry = &feature["geometry"]["coordinates"];
    let point =
        point(geometry[1].as_f64(), geometry[0].as_f64()).ok_or("Invalid USGS epicenter.")?;
    let depth_km = geometry[2].as_f64().ok_or("USGS did not report a depth.")?;
    let time_ms = props["time"].as_i64().ok_or("Invalid USGS origin time.")?;
    if DateTime::<Utc>::from_timestamp_millis(time_ms).is_none() {
        return Err("Invalid USGS origin time.".into());
    }
    let preferred = props["net"].as_str();
    let mut products: Vec<_> = props["products"]["phase-data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|product| product["status"].as_str() != Some("DELETE"))
        .filter_map(|product| {
            let url =
                reqwest::Url::parse(product["contents"]["quakeml.xml"]["url"].as_str()?).ok()?;
            if url.scheme() != "https" || url.host_str() != Some("earthquake.usgs.gov") {
                return None;
            }
            let matching = preferred.is_some() && product["source"].as_str() == preferred;
            let weight = product["preferredWeight"].as_i64().unwrap_or(0);
            let updated = product["updateTime"].as_i64().unwrap_or(0);
            Some(((matching, weight, updated), url))
        })
        .collect();
    products.sort_by_key(|(key, _)| std::cmp::Reverse(*key));
    if products.is_empty() {
        return Err("USGS has not published phase picks for this event.".into());
    }
    Ok((
        title,
        Origin {
            point,
            time_ms,
            depth_km,
        },
        products.into_iter().map(|(_, url)| url).collect(),
    ))
}

fn read(request: RequestBuilder, label: &str) -> Result<String, String> {
    let response = request
        .send()
        .map_err(|error| format!("{label} request failed: {error}"))?;
    let response = response
        .error_for_status()
        .map_err(|error| format!("{label} request failed: {error}"))?;
    let mut body = String::new();
    response
        .take(RESPONSE_LIMIT + 1)
        .read_to_string(&mut body)
        .map_err(|error| format!("{label} response could not be read: {error}"))?;
    if body.len() as u64 > RESPONSE_LIMIT {
        return Err(format!("{label} response is too large."));
    }
    Ok(body)
}

type Picks = BTreeMap<StationId, Vec<Pick>>;

fn phase_picks(xml: &str) -> Result<Picks, String> {
    let doc = Document::parse(xml).map_err(|_| "Invalid USGS QuakeML phase data.")?;
    let event = doc
        .descendants()
        .find(|node| node.has_tag_name("event"))
        .ok_or("QuakeML has no event.")?;
    let mut origins = event.children().filter(|node| node.has_tag_name("origin"));
    let preferred = field(event, "preferredOriginID");
    let origin = origins
        .clone()
        .find(|node| preferred.is_some() && node.attribute("publicID") == preferred)
        .or_else(|| origins.find(|node| child(*node, "arrival").is_some()))
        .ok_or("QuakeML has no origin with station arrivals.")?;

    let by_id: HashMap<_, _> = event
        .children()
        .filter(|node| node.has_tag_name("pick"))
        .filter_map(|pick| Some((pick.attribute("publicID")?, pick)))
        .collect();
    let mut picks = Picks::new();
    for arrival in origin
        .children()
        .filter(|node| node.has_tag_name("arrival"))
    {
        let Some(id) = field(arrival, "pickID") else {
            continue;
        };
        let Some(&pick) = by_id.get(id) else { continue };
        let Some(waveform) = child(pick, "waveformID") else {
            continue;
        };
        let Some(station) = station_id(
            waveform.attribute("networkCode"),
            waveform.attribute("stationCode"),
        ) else {
            continue;
        };
        let Some(time_ms) = child(pick, "time")
            .and_then(|node| field(node, "value"))
            .and_then(parse_utc)
            .map(|time| time.timestamp_millis())
        else {
            continue;
        };
        let Some(phase) = field(arrival, "phase").or_else(|| field(pick, "phaseHint")) else {
            continue;
        };
        if phase.is_empty() || phase.len() > 20 {
            continue;
        }
        let used = field(arrival, "timeWeight")
            .and_then(|value| value.parse::<f64>().ok())
            .is_none_or(|weight| weight > 0.0);
        picks.entry(station).or_default().push(Pick {
            phase: phase.to_owned(),
            time_ms,
            used,
        });
    }
    for arrivals in picks.values_mut() {
        arrivals.sort_by_key(|pick| pick.time_ms);
        // Several channels can publish the same phase timestamp at one station.
        let mut seen = BTreeSet::new();
        arrivals.retain(|pick| seen.insert((pick.time_ms, pick.phase.clone(), pick.used)));
    }
    Ok(picks)
}

fn child<'a, 'input: 'a>(node: Node<'a, 'input>, name: &str) -> Option<Node<'a, 'input>> {
    node.children().find(|child| child.has_tag_name(name))
}

fn field<'a, 'input: 'a>(node: Node<'a, 'input>, name: &str) -> Option<&'a str> {
    child(node, name)?.text().map(str::trim)
}

fn parse_utc(time: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(time)
        .or_else(|_| DateTime::parse_from_rfc3339(&format!("{time}Z")))
        .ok()
        .map(|time| time.with_timezone(&Utc))
}

fn point(lat: Option<f64>, lon: Option<f64>) -> Option<GeoPoint> {
    let (lat, lon) = (lat?, lon?);
    ((-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lon)).then_some(GeoPoint {
        lat: lat as f32,
        lon: lon as f32,
    })
}

fn station_id(network: Option<&str>, station: Option<&str>) -> Option<StationId> {
    fn token(text: &str) -> bool {
        !text.is_empty()
            && text.len() <= 12
            && text.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
    }
    let (network, code) = (network?, station?);
    (token(network) && token(code)).then(|| StationId {
        network: network.to_owned(),
        code: code.to_owned(),
    })
}

fn fetch_stations(
    client: &Client,
    service: &str,
    keys: &[StationId],
    time: DateTime<Utc>,
) -> Result<HashMap<StationId, Station>, String> {
    let mut request = String::from("level=station\n");
    let start = (time - chrono::TimeDelta::seconds(1)).to_rfc3339_opts(SecondsFormat::Millis, true);
    let end = (time + chrono::TimeDelta::seconds(1)).to_rfc3339_opts(SecondsFormat::Millis, true);
    for id in keys {
        writeln!(request, "{} {} * * {} {}", id.network, id.code, start, end)
            .expect("writing to a String");
    }
    let body = read(
        client
            .post(service)
            .header(CONTENT_TYPE, "text/plain")
            .body(request),
        "FDSN station metadata",
    )?;
    if body.is_empty() {
        return Ok(HashMap::new());
    }
    inventory(&body, time)
}

fn inventory(xml: &str, time: DateTime<Utc>) -> Result<HashMap<StationId, Station>, String> {
    let doc = Document::parse(xml).map_err(|_| "Invalid FDSN StationXML response.")?;
    let stations = doc
        .descendants()
        .filter(|node| node.has_tag_name("Station"))
        .filter_map(|node| {
            let network = node.parent_element()?.attribute("code");
            let id = station_id(network, node.attribute("code"))?;
            let point = point(
                field(node, "Latitude").and_then(|value| value.parse().ok()),
                field(node, "Longitude").and_then(|value| value.parse().ok()),
            )?;
            let elevation = field(node, "Elevation")?.parse::<f64>().ok()?;
            if !(-12_000.0..=10_000.0).contains(&elevation) {
                return None;
            }
            let start = node.attribute("startDate").map(parse_utc);
            let end = node.attribute("endDate").map(parse_utc);
            if start.is_some_and(|start| start.is_none_or(|start| start > time))
                || end.is_some_and(|end| end.is_none_or(|end| end < time))
            {
                return None;
            }
            Some((
                id,
                Station {
                    point,
                    elevation_km: elevation / 1000.0,
                },
            ))
        })
        .collect();
    Ok(stations)
}

fn assemble(
    title: String,
    origin: Origin,
    picks: Picks,
    stations: &HashMap<StationId, Station>,
) -> Result<Recording, String> {
    let total_stations = picks.len();
    let observations: Vec<_> = picks
        .into_iter()
        .filter_map(|(id, picks)| {
            let location = *stations.get(&id)?;
            // Reject plainly corrupted timestamps while keeping late teleseismic phases.
            let picks: Vec<_> = picks
                .into_iter()
                .filter(|pick| (-2_000..=5_400_000).contains(&(pick.time_ms - origin.time_ms)))
                .collect();
            (!picks.is_empty()).then_some(Observation {
                station: id,
                location,
                picks,
            })
        })
        .collect();
    if observations.is_empty() {
        return Err(
            "No independently geolocated stations matched the phase data for this event.".into(),
        );
    }
    let fit = locate::locate(&origin, &observations);
    Ok(Recording {
        title,
        origin,
        stations: observations,
        total_stations,
        fit,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uses_preferred_origin_picks_and_independent_active_station_epoch() {
        let xml = r#"<q:quakeml xmlns:q="http://quakeml.org/xmlns/quakeml/1.2" xmlns="http://quakeml.org/xmlns/bed/1.2">
            <eventParameters><event><preferredOriginID>preferred</preferredOriginID>
              <origin publicID="alternate"><arrival><pickID>alt</pickID><phase>P</phase></arrival></origin>
              <origin publicID="preferred">
                <arrival><pickID>p1</pickID><phase>Pg</phase><distance>25</distance><azimuth>80</azimuth></arrival>
                <arrival><pickID>s1</pickID><phase>Sg</phase><timeWeight>0</timeWeight></arrival>
              </origin>
              <pick publicID="alt"><time><value>2024-04-05T14:23:21Z</value></time>
                <waveformID networkCode="US" stationCode="ALT"/></pick>
              <pick publicID="p1"><time><value>2024-04-05T14:23:24.123Z</value></time>
                <waveformID networkCode="US" stationCode="ABC" channelCode="BHZ"/></pick>
              <pick publicID="s1"><time><value>2024-04-05T14:23:27.250Z</value></time>
                <waveformID networkCode="US" stationCode="ABC" channelCode="BHE"/></pick>
            </event></eventParameters></q:quakeml>"#;
        let stations = r#"<FDSNStationXML xmlns="http://www.fdsn.org/xml/station/1"><Network code="US">
            <Station code="ABC" startDate="2010-01-01T00:00:00" endDate="2020-01-01T00:00:00">
              <Latitude>20</Latitude><Longitude>-50</Longitude><Elevation>200</Elevation></Station>
            <Station code="ABC" startDate="2021-01-01T00:00:00">
              <Latitude>40.7</Latitude><Longitude>-74.4</Longitude><Elevation>110</Elevation></Station>
          </Network></FDSNStationXML>"#;
        let time = parse_utc("2024-04-05T14:23:20Z").unwrap();
        let picks = phase_picks(xml).unwrap();
        let stations = inventory(stations, time).unwrap();
        let origin = Origin {
            point: GeoPoint {
                lat: 40.5,
                lon: -74.5,
            },
            time_ms: time.timestamp_millis(),
            depth_km: 4.7,
        };
        let replay = assemble("Example".into(), origin, picks, &stations).unwrap();
        assert_eq!(replay.total_stations, 1);
        let station = &replay.stations[0];
        assert_eq!(station.location.point.lat, 40.7);
        assert_eq!(station.location.point.lon, -74.4);
        assert_eq!(station.picks[0].phase, "Pg");
        assert_eq!(replay.offset_s(station.picks[0].time_ms), 4.123);
        assert!(!station.picks[1].used);
        assert!(replay.fit.is_err());
    }

    #[test]
    fn chooses_same_catalog_product_before_other_origins() {
        let product = |source, weight| {
            serde_json::json!({
                "source":source, "preferredWeight":weight,
                "contents":{"quakeml.xml":{"url":format!("https://earthquake.usgs.gov/{source}.xml")}}
            })
        };
        let detail = serde_json::json!({
            "geometry":{"coordinates":[35.0,24.0,10.0]},
            "properties":{"type":"earthquake","net":"ci","time":1712340100000_i64,
                "products":{"phase-data":[product("us",500_i32),product("ci",300_i32)]}}
        });
        let (_, origin, products) = event(&detail).unwrap();
        assert_eq!(origin.point.lon, 35.0);
        assert_eq!(products[0].path(), "/ci.xml");
    }
}
