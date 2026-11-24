use super::{DAY, Epoch, Inventory, Update, cache};
use crate::station_replay::{Pick, StationId, source};
use chrono::{DateTime, SecondsFormat};
use reqwest::blocking::{Client, RequestBuilder};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

pub(super) fn load(
    from: i64,
    to: i64,
    cancelled: &AtomicBool,
    send: &(impl Fn(Update) + Sync),
) -> Result<(), String> {
    let stations = load_inventory(from, to, cancelled, |warning| send(Update::Warning(warning)))?;
    send(Update::Inventory(stations));
    load_picks(from, to, cancelled, send)
}

pub(in crate::station_replay) fn load_inventory(
    from: i64,
    to: i64,
    cancelled: &AtomicBool,
    mut warn: impl FnMut(String),
) -> Result<Inventory, String> {
    let client = source::client()?;
    let mut stations = Inventory::new();
    // Request whole-day station epochs even when today's playback ends at the current time.
    let day = DateTime::from_timestamp(from, 0).unwrap().date_naive();
    let start_date = day.to_string();
    let end_date = day.succ_opt().unwrap().to_string();
    for (name, service) in [
        ("EarthScope", source::STATIONS),
        ("ORFEUS", source::EIDA_STATIONS),
    ] {
        source::check_active(cancelled)?;
        let key = format!("inventory/{name}/{day}");
        if let Some(bytes) = cache::get(&key, 1, DAY)
            && let Ok(text) = std::str::from_utf8(&bytes)
            && parse_inventory(text, &mut stations).is_ok()
        {
            continue;
        }
        let query = client.get(service).query(&[
            ("level", "station"),
            ("format", "text"),
            ("starttime", &start_date),
            ("endtime", &end_date),
        ]);
        match source::read(query, name).and_then(|body| {
            parse_inventory(&body, &mut stations)?;
            cache::put(&key, 1, body.as_bytes());
            Ok(())
        }) {
            Ok(()) => {}
            Err(error) => warn(format!("{name} inventory incomplete · {error}")),
        }
    }
    stations.retain(|_, station| {
        station
            .epochs
            .retain(|epoch| epoch.start_ms < to * 1000 && epoch.end_ms >= from * 1000);
        // Prefer the latest valid epoch. A tie prefers the first provider (EarthScope).
        station
            .epochs
            .sort_by_key(|epoch| std::cmp::Reverse(epoch.start_ms));
        !station.epochs.is_empty()
    });
    if stations.is_empty() {
        return Err("No station coordinates available for this UTC day.".into());
    }
    Ok(stations)
}

pub(in crate::station_replay) fn load_picks(
    from: i64,
    to: i64,
    cancelled: &AtomicBool,
    send: &(impl Fn(Update) + Sync),
) -> Result<(), String> {
    let client = source::client()?;
    static NEXT_REQUEST: OnceLock<Mutex<Instant>> = OnceLock::new();
    let archive = UsgsArchive {
        client: &client,
        cancelled,
        next_request: NEXT_REQUEST.get_or_init(|| Mutex::new(Instant::now())),
        send,
        stopped: AtomicBool::new(false),
    };
    let events = catalog(&archive, from, to)?;
    send(Update::Catalog(events.len()));
    let mut missing = Vec::new();
    for event in events {
        source::check_active(cancelled)?;
        if let Some((origin_ms, picks)) = event.cached_picks() {
            send(Update::Picks {
                id: event.id,
                origin_ms,
                picks,
            });
        } else {
            missing.push(event);
        }
    }

    // Bound parallel downloads and reuse connections across events.
    const WORKERS: usize = 4;
    std::thread::scope(|scope| {
        for worker in 0..WORKERS.min(missing.len()) {
            let missing = &missing;
            let archive = &archive;
            scope.spawn(move || {
                for event in missing.iter().skip(worker).step_by(WORKERS) {
                    if cancelled.load(Ordering::Relaxed) || archive.stopped.load(Ordering::Relaxed)
                    {
                        break;
                    }
                    let update = match archive.picks(event) {
                        Ok((origin_ms, picks)) => Update::Picks {
                            id: event.id.clone(),
                            origin_ms,
                            picks,
                        },
                        Err(error) => Update::FailedEvent(format!("{} · {error}", event.id)),
                    };
                    send(update);
                }
            });
        }
    });
    if archive.stopped.load(Ordering::Relaxed) {
        return Err("USGS is still rate limiting downloads. Load the day again to resume from cached arrivals.".into());
    }
    Ok(())
}

fn utc(seconds: i64) -> String {
    DateTime::from_timestamp(seconds, 0)
        .unwrap()
        .to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn parse_inventory(body: &str, stations: &mut Inventory) -> Result<(), String> {
    if body.trim().is_empty() {
        return Ok(()); // FDSN 204 (no data)
    }
    if !body
        .lines()
        .next()
        .is_some_and(|line| line.starts_with("#Network"))
    {
        return Err("Unrecognized FDSN station table.".into());
    }
    for line in body.lines().filter(|line| !line.starts_with('#')) {
        let cols: Vec<_> = line.split('|').map(str::trim).collect();
        let [net, sta, lat, lon, _, _, start, end] = cols[..] else {
            continue;
        };
        let Some(id) = source::station_id(Some(net), Some(sta)) else {
            continue;
        };
        let Some(point) = source::point(lat.parse().ok(), lon.parse().ok()) else {
            continue;
        };
        let bound = |value: &str, open| {
            if value.is_empty() {
                Some(open)
            } else {
                source::parse_utc(value).map(|t| t.timestamp_millis())
            }
        };
        if let (Some(start_ms), Some(end_ms)) = (bound(start, i64::MIN), bound(end, i64::MAX))
            && start_ms <= end_ms
        {
            stations.entry(id).or_default().epochs.push(Epoch {
                point,
                start_ms,
                end_ms,
            });
        }
    }
    Ok(())
}

// Share one pacing and retry budget across concurrent USGS downloads.
struct UsgsArchive<'a> {
    client: &'a Client,
    cancelled: &'a AtomicBool,
    next_request: &'a Mutex<Instant>,
    send: &'a (dyn Fn(Update) + Sync),
    stopped: AtomicBool,
}

impl UsgsArchive<'_> {
    fn wait_turn(&self) -> Result<(), String> {
        loop {
            source::check_active(self.cancelled)?;
            if self.stopped.load(Ordering::Relaxed) {
                return Err("USGS rate limit reached.".into());
            }
            let delay = {
                let mut next = self.next_request.lock().unwrap();
                let now = Instant::now();
                if *next <= now {
                    *next = now + Duration::from_millis(250);
                    return Ok(());
                }
                *next - now
            };
            std::thread::sleep(delay.min(Duration::from_millis(200)));
        }
    }

    fn read(&self, request: RequestBuilder, label: &str) -> Result<String, String> {
        for attempt in 0..3 {
            self.wait_turn()?;
            let response = request
                .try_clone()
                .expect("archive GET")
                .send()
                .map_err(|e| format!("{label} request failed: {e}"))?;
            if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
                let retry = response
                    .headers()
                    .get(reqwest::header::RETRY_AFTER)
                    .and_then(|h| h.to_str().ok())
                    .and_then(|s| {
                        s.parse::<u64>().ok().or_else(|| {
                            chrono::DateTime::parse_from_rfc2822(s).ok().map(|t| {
                                (t.timestamp() - chrono::Utc::now().timestamp()).max(0) as u64
                            })
                        })
                    })
                    .unwrap_or(60 << attempt);
                let mut next = self.next_request.lock().unwrap();
                *next = (*next).max(
                    Instant::now()
                        .checked_add(Duration::from_secs(retry))
                        .unwrap_or(*next),
                );
                (self.send)(Update::Cooldown(*next));
                if attempt < 2 {
                    continue;
                }
                self.stopped.store(true, Ordering::Relaxed);
            }
            return source::read_response(response, label);
        }
        unreachable!()
    }

    fn picks(&self, event: &CatalogEvent) -> Result<(i64, source::Picks), String> {
        let query = self
            .client
            .get(source::USGS_QUERY)
            .query(&[("eventid", &*event.id), ("format", "geojson")]);
        let detail = self.read(query, "USGS earthquake")?;
        let (_, origin, picks) = source::product_picks(&detail, self.cancelled, |url| {
            self.read(self.client.get(url), "USGS phase data")
        })?;
        if let Ok(bytes) = serde_json::to_vec(&(origin.time_ms, picks.iter().collect::<Vec<_>>())) {
            cache::put(&event.cache_key(), event.updated, &bytes);
        }
        Ok((origin.time_ms, picks))
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct CatalogEvent {
    id: String,
    updated: i64,
}

impl CatalogEvent {
    fn cache_key(&self) -> String {
        format!("phase/{}", self.id)
    }

    fn cached_picks(&self) -> Option<(i64, source::Picks)> {
        type CachedPicks = (i64, Vec<(StationId, Vec<Pick>)>);
        let bytes = cache::get(&self.cache_key(), self.updated, 30 * DAY)?;
        let (origin, picks) = serde_json::from_slice::<CachedPicks>(&bytes).ok()?;
        Some((origin, picks.into_iter().collect()))
    }
}

fn catalog(archive: &UsgsArchive<'_>, from: i64, to: i64) -> Result<Vec<CatalogEvent>, String> {
    // Today's endpoint moves. Cache completed day catalogs and reuse event revisions across both.
    let historical = to - from == DAY;
    let key = format!("catalog/{from}");
    if historical
        && let Some(bytes) = cache::get(&key, 1, 6 * 3600)
        && let Ok(events) = serde_json::from_slice(&bytes)
    {
        return Ok(events);
    }
    const PAGE: usize = 500;
    // Query origin times on both sides of midnight, then select by the pick's timestamp.
    let start = utc(from - source::MAX_PICK_MS / 1000);
    let end = utc(to - source::MIN_PICK_MS / 1000);
    let mut seen = HashSet::new();
    let mut ids = Vec::new();
    let mut offset = 1;
    loop {
        source::check_active(archive.cancelled)?;
        let query = archive.client.get(source::USGS_QUERY).query(&[
            ("format", "geojson"),
            ("eventtype", "earthquake"),
            ("producttype", "phase-data"),
            ("orderby", "time-asc"),
            ("starttime", &start),
            ("endtime", &end),
            ("limit", &PAGE.to_string()),
            ("offset", &offset.to_string()),
        ]);
        let body = archive.read(query, "USGS phase catalog")?;
        if body.is_empty() {
            break;
        }
        let json: serde_json::Value =
            serde_json::from_str(&body).map_err(|_| "Invalid USGS phase catalog.")?;
        let features = json["features"]
            .as_array()
            .ok_or("Missing USGS catalog features.")?;
        for feature in features {
            let id = feature["id"].as_str().ok_or("Missing USGS event ID.")?;
            if id.is_empty()
                || !id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            {
                return Err("Invalid USGS catalog event ID.".into());
            }
            if seen.insert(id.to_owned()) {
                let updated = feature["properties"]["updated"]
                    .as_i64()
                    .ok_or("Missing USGS event revision.")?;
                ids.push(CatalogEvent {
                    id: id.to_owned(),
                    updated,
                });
            }
        }
        if features.len() < PAGE {
            break;
        }
        offset += features.len();
    }
    if historical && let Ok(bytes) = serde_json::to_vec(&ids) {
        cache::put(&key, 1, &bytes);
    }
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retries_rate_limited_archive_request_and_reports_cooldown() {
        use std::io::{BufRead, BufReader, Write};
        use std::net::TcpListener;
        let server = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", server.local_addr().unwrap());
        let worker = std::thread::spawn(move || {
            for reply in [
                "429 Too Many Requests\r\nRetry-After: 0\r\nContent-Length: 0\r\n\r\n",
                "200 OK\r\nContent-Length: 2\r\n\r\nok",
            ] {
                let (mut socket, _) = server.accept().unwrap();
                let mut reader = BufReader::new(&socket);
                let mut line = String::new();
                while reader.read_line(&mut line).unwrap() > 0 && line != "\r\n" {
                    line.clear();
                }
                write!(socket, "HTTP/1.1 {reply}").unwrap();
            }
        });
        let client = Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap();
        let notified = AtomicBool::new(false);
        let archive = UsgsArchive {
            client: &client,
            cancelled: &AtomicBool::new(false),
            next_request: &Mutex::new(Instant::now()),
            send: &|msg| {
                if matches!(msg, Update::Cooldown(_)) {
                    notified.store(true, Ordering::Relaxed);
                }
            },
            stopped: AtomicBool::new(false),
        };
        assert_eq!(archive.read(client.get(url), "test").unwrap(), "ok");
        assert!(notified.load(Ordering::Relaxed));
        worker.join().unwrap();
    }

    #[test]
    fn station_text_preserves_quiet_stations_and_dated_locations() {
        let mut stations = Inventory::new();
        parse_inventory("#Network|Station|Latitude|Longitude|Elevation|SiteName|StartTime|EndTime\nXX|ONE|30|40|0|Old site||2024-04-05T09:00:00\nXX|ONE|31|41|0|New site|2024-04-05T10:00:00|\nYY|ONE|32|42|0|Other network||\nXX|BROKEN|300|40|0|Bad||", &mut stations).unwrap();
        assert_eq!(stations.len(), 2);
        let id = source::station_id(Some("XX"), Some("ONE")).unwrap();
        let loc = |at| stations[&id].location(source::parse_utc(at).unwrap().timestamp_millis());
        assert_eq!(loc("2024-04-05T08:00:00Z").unwrap().lat, 30.);
        assert!(loc("2024-04-05T09:30:00Z").is_none());
        assert_eq!(loc("2024-04-05T11:00:00Z").unwrap().lat, 31.);
        assert!(parse_inventory("<html>Service unavailable</html>", &mut stations).is_err());
    }
}
