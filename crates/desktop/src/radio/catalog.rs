//! Bounded, on-demand discovery through public radio catalogs and direct-stream publishers.
use super::{RadioFeed, RadioKind, restricted_provider, stream_url, valid_coordinates};
use reqwest::blocking::Client;
use serde::Deserialize;
use std::collections::HashSet;
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const MAX_BODY: u64 = 4 * 1024 * 1024;
const QUERIES: &[(&str, &str)] = &[
    ("tag", "scanner"),
    ("tag", "public safety"),
    ("tag", "police"),
    ("tag", "fire"),
    ("tag", "ems"),
    ("tag", "air traffic control"),
    ("tag", "aviation"),
    ("tag", "marine"),
    ("tag", "weather"),
    ("name", "scanner"),
    ("name", "dispatch"),
    ("name", "fire rescue"),
];

pub(crate) struct CatalogReport {
    pub feeds: Vec<RadioFeed>,
    pub warning: Option<String>,
}

#[derive(Deserialize)]
struct Server {
    name: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Station {
    stationuuid: String,
    name: String,
    url: String,
    url_resolved: String,
    homepage: String,
    tags: String,
    state: String,
    countrycode: String,
    geo_lat: Option<f64>,
    geo_long: Option<f64>,
    lastcheckok: i64,
}

fn client() -> Result<Client, String> {
    Client::builder()
        .user_agent("1kEE/0.1 public-radio-browser")
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(12))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| e.to_string())
}

pub(super) fn get_json<T: serde::de::DeserializeOwned>(
    client: &Client,
    url: reqwest::Url,
) -> Result<T, String> {
    let response = client
        .get(url)
        .send()
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?;
    let mut body = Vec::new();
    response
        .take(MAX_BODY + 1)
        .read_to_end(&mut body)
        .map_err(|e| e.to_string())?;
    if body.len() as u64 > MAX_BODY {
        return Err("Radio directory response is too large.".into());
    }
    serde_json::from_slice(&body).map_err(|e| format!("Invalid radio directory response: {e}"))
}

fn servers(client: &Client) -> Vec<String> {
    // Discover current mirrors rather than pinning every request to one server.
    let mut found = Vec::new();
    for host in ["all.api.radio-browser.info", "de1.api.radio-browser.info"] {
        let url = reqwest::Url::parse(&format!("https://{host}/json/servers")).unwrap();
        if let Ok(entries) = get_json::<Vec<Server>>(client, url) {
            found.extend(entries.into_iter().map(|s| s.name).filter(|name| {
                name.ends_with(".api.radio-browser.info")
                    && name
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'))
            }));
            if !found.is_empty() {
                break;
            }
        }
    }
    found.sort();
    found.dedup();
    if found.is_empty() {
        found.push("de1.api.radio-browser.info".into());
    }
    let rotation = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as usize
        % found.len();
    found.rotate_left(rotation);
    found.truncate(3);
    found
}

pub(crate) fn fetch(cancel: &AtomicBool) -> Result<CatalogReport, String> {
    let client = client()?;
    let hosts = servers(&client);
    let mut feeds = Vec::new();
    let mut ids = HashSet::new();
    let mut urls = HashSet::new();
    let mut failures = 0;
    let mut successes = 0;
    for feed in super::community::feeds() {
        ids.insert(feed.id.clone());
        urls.insert(feed.url.clone());
        feeds.push(feed);
    }
    if !cancel.load(Ordering::Acquire) {
        match super::weather_radio::fetch(&client) {
            Ok(weather) => {
                for feed in weather {
                    if urls.insert(feed.url.clone()) {
                        ids.insert(feed.id.clone());
                        feeds.push(feed);
                    }
                }
            }
            Err(_) => failures += 1,
        }
    }
    for &(field, term) in QUERIES {
        if cancel.load(Ordering::Acquire) {
            return Err("Search cancelled.".into());
        }
        let mut result = None;
        for host in &hosts {
            if cancel.load(Ordering::Acquire) {
                return Err("Search cancelled.".into());
            }
            let mut url =
                reqwest::Url::parse(&format!("https://{host}/json/stations/search")).unwrap();
            url.query_pairs_mut()
                .append_pair(field, term)
                .append_pair("hidebroken", "true")
                .append_pair("limit", "500")
                .append_pair("order", "name");
            if let Ok(stations) = get_json::<Vec<Station>>(&client, url) {
                // A full page is intentionally reported as incomplete, never silently global coverage.
                if stations.len() == 500 {
                    failures += 1;
                }
                result = Some(stations);
                break;
            }
        }
        match result {
            Some(stations) => {
                successes += 1;
                for station in stations {
                    if let Some(feed) = station.into_feed()
                        && !ids.contains(&feed.id)
                        && !urls.contains(&feed.url)
                    {
                        ids.insert(feed.id.clone());
                        urls.insert(feed.url.clone());
                        feeds.push(feed);
                    }
                }
            }
            None => failures += 1,
        }
    }
    if successes == 0 {
        failures += 1;
    }
    Ok(CatalogReport { feeds, warning: (failures > 0).then(||
        "Some radio sources were unavailable or reached their limit. Results may be incomplete; bundled community feeds are still available.".into()) })
}

impl Station {
    fn into_feed(self) -> Option<RadioFeed> {
        if self.stationuuid.is_empty() || self.name.trim().is_empty() || self.lastcheckok != 1 {
            return None;
        }
        let kinds = classify(&self.name, &self.tags);
        if kinds.is_empty() {
            return None;
        }
        let url = if self.url_resolved.trim().is_empty() {
            &self.url
        } else {
            &self.url_resolved
        };
        let url = stream_url(url).ok()?.to_string();
        if reqwest::Url::parse(&self.homepage).is_ok_and(|u| restricted_provider(&u)) {
            return None;
        }
        let (latitude, longitude) = match (self.geo_lat, self.geo_long) {
            (Some(lat), Some(lon)) if valid_coordinates(lat, lon) => (Some(lat), Some(lon)),
            _ => (None, None),
        };
        Some(RadioFeed {
            id: format!("rb:{}", self.stationuuid),
            directory_id: Some(self.stationuuid),
            name: self.name,
            url,
            homepage: self.homepage,
            kinds,
            latitude,
            longitude,
            locality: [self.state, self.countrycode]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(", "),
        })
    }
}

/// Only explicit dispatch/safety categories qualify; bands named "The Police" do not.
pub(crate) fn classify(name: &str, tags: &str) -> Vec<RadioKind> {
    let name = name.to_lowercase();
    let tags: Vec<_> = tags.split(',').map(|s| s.trim().to_lowercase()).collect();
    let tag = |parts: &[&str]| {
        tags.iter()
            .any(|t| parts.iter().any(|p| t == p || t.contains(p)))
    };
    let mut kinds = Vec::new();
    if tag(&["police scanner", "police dispatch"])
        || tags.iter().any(|s| s == "police")
        || name.contains("police scanner")
        || name.contains("police dispatch")
    {
        kinds.push(RadioKind::Police);
    }
    if tag(&[
        "fire scanner",
        "fire dispatch",
        "fire rescue",
        "fire department",
    ]) || tags.iter().any(|s| s == "fire")
        || name.contains("fire rescue")
        || name.contains("fire dispatch")
        || name.contains("fire scanner")
    {
        kinds.push(RadioKind::Fire);
    }
    if tag(&["ambulance", "emergency medical"])
        || tags.iter().any(|s| s == "ems")
        || name.contains("ems dispatch")
        || name.contains("ambulance dispatch")
    {
        kinds.push(RadioKind::Ems);
    }
    if tag(&["public safety", "emergency", "scanner"])
        || name.contains("dispatch")
        || name.contains("public safety")
    {
        kinds.push(RadioKind::PublicSafety);
    }
    if tag(&["air traffic", "aviation scanner"])
        || tags
            .iter()
            .any(|s| matches!(s.as_str(), "atc" | "aviation"))
        || name.contains("air traffic control")
    {
        kinds.push(RadioKind::Aviation);
    }
    if tag(&["marine scanner", "marine radio", "coast guard"])
        || tags.iter().any(|s| s == "marine")
        || name.contains("marine scanner")
    {
        kinds.push(RadioKind::Marine);
    }
    if tag(&["noaa", "weather radio", "weather alert"]) || name.contains("noaa weather") {
        kinds.push(RadioKind::Weather);
    }
    kinds
}

/// Directory etiquette: count an explicit Play click, never background browsing.
pub(crate) fn record_play(uuid: String) {
    if uuid.len() != 36 || !uuid.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return;
    }
    std::thread::spawn(move || {
        if let Ok(client) = client() {
            let hosts = servers(&client);
            if let Some(host) = hosts.first() {
                let _ = client.get(format!("https://{host}/json/url/{uuid}")).send();
            }
        }
    });
}

#[cfg(test)]
pub(super) fn parse_fixture(body: &str) -> Vec<RadioFeed> {
    serde_json::from_str::<Vec<Station>>(body)
        .unwrap()
        .into_iter()
        .filter_map(Station::into_feed)
        .collect()
}
