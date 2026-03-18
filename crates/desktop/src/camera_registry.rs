use crate::camera_scrape_catalog::{self, ScrapedCameraSource, ScrapedCameraSourceKind};
use crate::camera_source_catalog::{self, PublicCameraSource, PublicCameraSourceKind};
use crate::model::{AppModel, CameraConnectionState, CameraFeed, GeoPoint};
use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, HeaderValue};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const POLL_INTERVAL: Duration = Duration::from_secs(300);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const WINDY_BBOX_HALF_SPAN_DEG: f32 = 1.8;

struct PollManager {
    active: Option<ActivePoll>,
    generation: u64,
    tracked_signature: String,
    next_poll_at: Option<Instant>,
    shutdown: bool,
}

struct ActivePoll {
    generation: u64,
    handle: JoinHandle<PollOutcome>,
}

enum PollOutcome {
    Success {
        cameras: Vec<CameraFeed>,
        source_label: String,
    },
    Error(String),
}

pub fn tick(model: &mut AppModel) {
    let signature = poll_signature(model);
    let now = Instant::now();
    let mut finished = None;

    {
        let mut manager = manager().lock().unwrap();
        if manager.tracked_signature != signature {
            manager.tracked_signature = signature.clone();
            manager.generation = manager.generation.wrapping_add(1);
            manager.next_poll_at = Some(now);
            manager.shutdown = false;
        }

        if let Some(active) = manager.active.as_ref() {
            if active.handle.is_finished() {
                finished = manager
                    .active
                    .take()
                    .map(|active| (active.generation, active.handle));
            }
        }
    }

    if let Some((generation, handle)) = finished {
        match handle.join() {
            Ok(outcome) => apply_outcome(model, generation, outcome),
            Err(_) => {
                if generation == current_generation() {
                    model.camera_registry_status = "error".into();
                    model.push_log("Camera registry worker panicked before returning data.".into());
                }
            }
        }
    }

    let public_sources = camera_source_catalog::load_public_sources(model.selected_root.as_deref());
    let scrape_sources = camera_scrape_catalog::load_scrape_sources(model.selected_root.as_deref());
    if !model.has_camera_source_keys() && public_sources.is_empty() && scrape_sources.is_empty() {
        if model.camera_registry_status != "demo" {
            model.camera_registry_status = "demo".into();
        }
        return;
    }

    let mut should_spawn = false;
    let mut spawn_generation = 0_u64;
    {
        let mut manager = manager().lock().unwrap();
        if !manager.shutdown
            && manager.active.is_none()
            && now >= manager.next_poll_at.unwrap_or(now)
        {
            should_spawn = true;
            spawn_generation = manager.generation;
            manager.next_poll_at = Some(now + POLL_INTERVAL);
        }
    }

    if should_spawn {
        let windy_key = model.windy_webcams_api_key.trim().to_owned();
        let ny511_key = model.ny511_api_key.trim().to_owned();
        let focus = model.terrain_focus_location();
        let public_sources = public_sources;
        let scrape_sources = scrape_sources;

        let handle = thread::spawn(move || {
            fetch_camera_registry(
                &windy_key,
                &ny511_key,
                focus,
                &public_sources,
                &scrape_sources,
            )
        });

        let mut manager = manager().lock().unwrap();
        if !manager.shutdown && manager.generation == spawn_generation {
            manager.active = Some(ActivePoll {
                generation: spawn_generation,
                handle,
            });
            model.camera_registry_status = "syncing".into();
        }
    }
}

pub fn invalidate() {
    let now = Instant::now();
    let mut manager = manager().lock().unwrap();
    manager.generation = manager.generation.wrapping_add(1);
    manager.next_poll_at = Some(now);
    manager.shutdown = false;
}

pub fn shutdown() {
    let mut manager = manager().lock().unwrap();
    manager.generation = manager.generation.wrapping_add(1);
    manager.shutdown = true;
    manager.next_poll_at = None;
}

fn apply_outcome(model: &mut AppModel, generation: u64, outcome: PollOutcome) {
    if generation != current_generation() {
        return;
    }

    match outcome {
        PollOutcome::Success {
            cameras,
            source_label,
        } => {
            if cameras.is_empty() {
                model.camera_registry_status = "empty".into();
                model.push_log(format!(
                    "Camera registry sync completed but returned no cameras from {source_label}."
                ));
            } else {
                model.replace_camera_registry(cameras, &source_label);
            }
        }
        PollOutcome::Error(error) => {
            model.camera_registry_status = "error".into();
            model.push_log(format!("Camera registry sync failed: {error}"));
        }
    }
}

fn poll_signature(model: &AppModel) -> String {
    let focus = model
        .terrain_focus_location()
        .map(|point| format!("{:.2}:{:.2}", point.lat, point.lon))
        .unwrap_or_else(|| "nofocus".into());
    let public_sources = camera_source_catalog::load_public_sources(model.selected_root.as_deref());
    let scrape_sources = camera_scrape_catalog::load_scrape_sources(model.selected_root.as_deref());
    format!(
        "windy:{}|511ny:{}|public:{}|scrape:{}|focus:{}",
        !model.windy_webcams_api_key.trim().is_empty(),
        !model.ny511_api_key.trim().is_empty(),
        source_signature(&public_sources),
        scrape_source_signature(&scrape_sources),
        focus
    )
}

fn fetch_camera_registry(
    windy_key: &str,
    ny511_key: &str,
    focus: Option<GeoPoint>,
    public_sources: &[PublicCameraSource],
    scrape_sources: &[ScrapedCameraSource],
) -> PollOutcome {
    let client = match Client::builder().timeout(REQUEST_TIMEOUT).build() {
        Ok(client) => client,
        Err(error) => return PollOutcome::Error(error.to_string()),
    };

    let mut cameras = Vec::new();
    let mut source_parts = Vec::new();

    if !ny511_key.trim().is_empty() {
        match fetch_511ny_cameras(&client, ny511_key.trim()) {
            Ok(mut fetched) => {
                source_parts.push("511NY".to_owned());
                cameras.append(&mut fetched);
            }
            Err(error) => {
                return PollOutcome::Error(format!("511NY adapter failed: {error}"));
            }
        }
    }

    if !windy_key.trim().is_empty() {
        if let Some(focus) = focus {
            match fetch_windy_cameras(&client, windy_key.trim(), focus) {
                Ok(mut fetched) => {
                    source_parts.push("Windy Webcams".to_owned());
                    cameras.append(&mut fetched);
                }
                Err(error) => {
                    return PollOutcome::Error(format!("Windy adapter failed: {error}"));
                }
            }
        }
    }

    for source in public_sources {
        match fetch_public_source(&client, source) {
            Ok(mut fetched) => {
                if !fetched.is_empty() {
                    source_parts.push(source.name.clone());
                    cameras.append(&mut fetched);
                }
            }
            Err(error) => {
                return PollOutcome::Error(format!(
                    "Public source '{}' failed: {error}",
                    source.name
                ));
            }
        }
    }

    for source in scrape_sources {
        match fetch_scraped_source(&client, source) {
            Ok(mut fetched) => {
                if !fetched.is_empty() {
                    source_parts.push(source.name.clone());
                    cameras.append(&mut fetched);
                }
            }
            Err(error) => {
                return PollOutcome::Error(format!(
                    "Scraped source '{}' failed: {error}",
                    source.name
                ));
            }
        }
    }

    dedupe_cameras(&mut cameras);

    PollOutcome::Success {
        cameras,
        source_label: if source_parts.is_empty() {
            "configured camera sources".into()
        } else {
            source_parts.join(" + ")
        },
    }
}

fn fetch_public_source(
    client: &Client,
    source: &PublicCameraSource,
) -> Result<Vec<CameraFeed>, String> {
    match source.kind {
        PublicCameraSourceKind::JsonArray => fetch_public_json_array_source(client, source),
        PublicCameraSourceKind::GeoJson => fetch_public_geojson_source(client, source),
        PublicCameraSourceKind::ArcGisFeatureService => fetch_public_arcgis_source(client, source),
    }
}

fn fetch_scraped_source(
    client: &Client,
    source: &ScrapedCameraSource,
) -> Result<Vec<CameraFeed>, String> {
    let response = client
        .get(&source.page_url)
        .send()
        .map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err(format!("unexpected status {}", response.status()));
    }

    let body = response.text().map_err(|error| error.to_string())?;
    let label = source
        .label_override
        .clone()
        .or_else(|| extract_scraped_label(source, &body))
        .unwrap_or_else(|| source.name.clone());
    let stream_url = source
        .stream_url_override
        .clone()
        .or_else(|| extract_scraped_stream_url(source, &body))
        .unwrap_or_else(|| source.page_url.clone());
    let (lat, lon) = source
        .latitude
        .zip(source.longitude)
        .or_else(|| extract_scraped_point(source, &body))
        .ok_or_else(|| "no coordinates were provided or found in page markup".to_owned())?;

    Ok(vec![CameraFeed {
        id: format!("{}-{}", slugify(&source.provider), slugify(&source.name)),
        label,
        provider: source.provider.clone(),
        kind: source.kind_value.clone().unwrap_or_else(|| "webcam".into()),
        location: GeoPoint { lat, lon },
        stream_url,
        last_seen: "scraped directory".into(),
        status: CameraConnectionState::Idle,
    }])
}

fn fetch_public_json_array_source(
    client: &Client,
    source: &PublicCameraSource,
) -> Result<Vec<CameraFeed>, String> {
    let response = client
        .get(&source.endpoint)
        .send()
        .map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err(format!("unexpected status {}", response.status()));
    }

    let body = response.text().map_err(|error| error.to_string())?;
    let payload: Value = serde_json::from_str(&body).map_err(|error| error.to_string())?;
    let records = if let Some(array_field) = source.array_field.as_deref() {
        payload
            .get(array_field)
            .and_then(Value::as_array)
            .ok_or_else(|| format!("array field '{array_field}' missing"))?
    } else {
        payload
            .as_array()
            .ok_or_else(|| "response was not a top-level array".to_owned())?
    };

    Ok(records
        .iter()
        .filter_map(|record| camera_from_record(source, record, None, None))
        .collect())
}

fn fetch_public_geojson_source(
    client: &Client,
    source: &PublicCameraSource,
) -> Result<Vec<CameraFeed>, String> {
    let response = client
        .get(&source.endpoint)
        .send()
        .map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err(format!("unexpected status {}", response.status()));
    }

    let body = response.text().map_err(|error| error.to_string())?;
    let payload: Value = serde_json::from_str(&body).map_err(|error| error.to_string())?;
    let features = payload
        .get("features")
        .and_then(Value::as_array)
        .ok_or_else(|| "geojson response did not contain features".to_owned())?;

    let mut out = Vec::new();
    for feature in features {
        let properties = feature
            .get("properties")
            .filter(|value| value.is_object())
            .unwrap_or(feature);
        let geometry = feature.get("geometry");
        let (lat, lon) = geometry
            .and_then(extract_geojson_point)
            .unwrap_or((None, None));
        if let Some(camera) = camera_from_record(source, properties, lat, lon) {
            out.push(camera);
        }
    }
    Ok(out)
}

fn fetch_public_arcgis_source(
    client: &Client,
    source: &PublicCameraSource,
) -> Result<Vec<CameraFeed>, String> {
    let query_url = if source.endpoint.contains('?') {
        format!(
            "{}&where=1%3D1&outFields=*&returnGeometry=true&f=json",
            source.endpoint
        )
    } else {
        format!(
            "{}?where=1%3D1&outFields=*&returnGeometry=true&f=json",
            source.endpoint
        )
    };

    let response = client
        .get(query_url)
        .send()
        .map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err(format!("unexpected status {}", response.status()));
    }

    let body = response.text().map_err(|error| error.to_string())?;
    let payload: Value = serde_json::from_str(&body).map_err(|error| error.to_string())?;
    let features = payload
        .get("features")
        .and_then(Value::as_array)
        .ok_or_else(|| "arcgis response did not contain features".to_owned())?;

    let mut out = Vec::new();
    for feature in features {
        let attributes = feature
            .get("attributes")
            .filter(|value| value.is_object())
            .unwrap_or(feature);
        let geometry = feature.get("geometry");
        let lat = geometry
            .and_then(|geometry| geometry.get("y"))
            .and_then(value_as_f32_ref)
            .or_else(|| {
                source
                    .geometry_y_field
                    .as_deref()
                    .and_then(|field| attributes.get(field))
                    .and_then(value_as_f32_ref)
            });
        let lon = geometry
            .and_then(|geometry| geometry.get("x"))
            .and_then(value_as_f32_ref)
            .or_else(|| {
                source
                    .geometry_x_field
                    .as_deref()
                    .and_then(|field| attributes.get(field))
                    .and_then(value_as_f32_ref)
            });

        if let Some(camera) = camera_from_record(source, attributes, lat, lon) {
            out.push(camera);
        }
    }
    Ok(out)
}

fn camera_from_record(
    source: &PublicCameraSource,
    record: &Value,
    lat_override: Option<f32>,
    lon_override: Option<f32>,
) -> Option<CameraFeed> {
    let id_field = source.id_field.as_deref().unwrap_or("id");
    let label_field = source.label_field.as_deref().unwrap_or("name");
    let lat_field = source.latitude_field.as_deref().unwrap_or("latitude");
    let lon_field = source.longitude_field.as_deref().unwrap_or("longitude");

    let id = record.get(id_field).and_then(value_as_string_ref)?;
    let label = record
        .get(label_field)
        .and_then(value_as_string_ref)
        .unwrap_or_else(|| source.name.clone());
    let lat = lat_override.or_else(|| record.get(lat_field).and_then(value_as_f32_ref))?;
    let lon = lon_override.or_else(|| record.get(lon_field).and_then(value_as_f32_ref))?;
    let stream_url = source
        .stream_url_field
        .as_deref()
        .and_then(|field| record.get(field))
        .and_then(value_as_string_ref)
        .unwrap_or_default();

    Some(CameraFeed {
        id: format!("{}-{id}", slugify(&source.provider)),
        label,
        provider: source.provider.clone(),
        kind: source.kind_value.clone().unwrap_or_else(|| "camera".into()),
        location: GeoPoint { lat, lon },
        stream_url,
        last_seen: "public source".into(),
        status: CameraConnectionState::Idle,
    })
}

fn extract_geojson_point(geometry: &Value) -> Option<(Option<f32>, Option<f32>)> {
    let coordinates = geometry.get("coordinates")?.as_array()?;
    if coordinates.len() < 2 {
        return None;
    }
    let lon = coordinates.first().and_then(value_as_f32_ref);
    let lat = coordinates.get(1).and_then(value_as_f32_ref);
    Some((lat, lon))
}

fn fetch_511ny_cameras(client: &Client, api_key: &str) -> Result<Vec<CameraFeed>, String> {
    let url = format!("https://511ny.org/api/v2/get/cameras?key={api_key}&format=json");
    let response = client.get(url).send().map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err(format!("unexpected status {}", response.status()));
    }

    let body = response.text().map_err(|error| error.to_string())?;
    let payload: Value = serde_json::from_str(&body).map_err(|error| error.to_string())?;
    let cameras = payload
        .as_array()
        .or_else(|| payload.get("cameras").and_then(Value::as_array))
        .ok_or_else(|| "response did not contain a camera array".to_owned())?;

    let mut out = Vec::new();
    for raw in cameras {
        let Some(id) = raw
            .get("Id")
            .or_else(|| raw.get("id"))
            .and_then(value_as_string)
        else {
            continue;
        };
        let Some(lat) = raw
            .get("Latitude")
            .or_else(|| raw.get("latitude"))
            .and_then(value_as_f32)
        else {
            continue;
        };
        let Some(lon) = raw
            .get("Longitude")
            .or_else(|| raw.get("longitude"))
            .and_then(value_as_f32)
        else {
            continue;
        };

        let roadway = raw
            .get("Roadway")
            .or_else(|| raw.get("roadway"))
            .and_then(value_as_string)
            .unwrap_or_else(|| "New York traffic camera".into());
        let direction = raw
            .get("Direction")
            .or_else(|| raw.get("direction"))
            .and_then(value_as_string)
            .unwrap_or_default();
        let source = raw
            .get("Source")
            .or_else(|| raw.get("source"))
            .and_then(value_as_string)
            .unwrap_or_else(|| "511NY".into());

        let mut stream_url = raw
            .get("Url")
            .or_else(|| raw.get("url"))
            .and_then(value_as_string)
            .unwrap_or_default();
        if stream_url.is_empty() {
            if let Some(views) = raw
                .get("Views")
                .or_else(|| raw.get("views"))
                .and_then(Value::as_array)
            {
                stream_url = views
                    .iter()
                    .filter_map(|view| {
                        view.get("Url")
                            .or_else(|| view.get("url"))
                            .and_then(value_as_string)
                    })
                    .find(|url| !url.is_empty())
                    .unwrap_or_default();
            }
        }

        out.push(CameraFeed {
            id: format!("511ny-{id}"),
            label: if direction.is_empty() {
                roadway.clone()
            } else {
                format!("{roadway} {direction}")
            },
            provider: source,
            kind: "traffic".into(),
            location: GeoPoint { lat, lon },
            stream_url,
            last_seen: "live sync".into(),
            status: CameraConnectionState::Idle,
        });
    }

    Ok(out)
}

fn fetch_windy_cameras(
    client: &Client,
    api_key: &str,
    focus: GeoPoint,
) -> Result<Vec<CameraFeed>, String> {
    let west = (focus.lon - WINDY_BBOX_HALF_SPAN_DEG).clamp(-180.0, 180.0);
    let east = (focus.lon + WINDY_BBOX_HALF_SPAN_DEG).clamp(-180.0, 180.0);
    let south = (focus.lat - WINDY_BBOX_HALF_SPAN_DEG).clamp(-85.0, 85.0);
    let north = (focus.lat + WINDY_BBOX_HALF_SPAN_DEG).clamp(-85.0, 85.0);

    let url = format!(
        "https://api.windy.com/webcams/api/v3/webcams?bbox={west},{south},{east},{north}&limit=200&include=location,urls,player"
    );

    let mut headers = HeaderMap::new();
    headers.insert(
        "x-windy-api-key",
        HeaderValue::from_str(api_key).map_err(|error| error.to_string())?,
    );

    let response = client
        .get(url)
        .headers(headers)
        .send()
        .map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err(format!("unexpected status {}", response.status()));
    }

    let body = response.text().map_err(|error| error.to_string())?;
    let payload: Value = serde_json::from_str(&body).map_err(|error| error.to_string())?;
    let webcams = payload
        .get("webcams")
        .and_then(Value::as_array)
        .or_else(|| {
            payload
                .get("result")
                .and_then(|result| result.get("webcams"))
                .and_then(Value::as_array)
        })
        .ok_or_else(|| "response did not contain a webcam array".to_owned())?;

    let mut out = Vec::new();
    for raw in webcams {
        let Some(id) = raw.get("id").and_then(value_as_string) else {
            continue;
        };

        let location = raw.get("location").unwrap_or(raw);
        let Some(lat) = location
            .get("latitude")
            .or_else(|| location.get("lat"))
            .and_then(value_as_f32)
        else {
            continue;
        };
        let Some(lon) = location
            .get("longitude")
            .or_else(|| location.get("lng"))
            .or_else(|| location.get("lon"))
            .and_then(value_as_f32)
        else {
            continue;
        };

        let label = raw
            .get("title")
            .or_else(|| raw.get("name"))
            .or_else(|| location.get("city"))
            .and_then(value_as_string)
            .unwrap_or_else(|| "Windy webcam".into());
        let stream_url = raw
            .get("player")
            .and_then(|player| player.get("live"))
            .and_then(value_as_string)
            .or_else(|| {
                raw.get("urls")
                    .and_then(|urls| urls.get("detail"))
                    .and_then(value_as_string)
            })
            .unwrap_or_default();

        out.push(CameraFeed {
            id: format!("windy-{id}"),
            label,
            provider: "Windy Webcams".into(),
            kind: "webcam".into(),
            location: GeoPoint { lat, lon },
            stream_url,
            last_seen: "live sync".into(),
            status: CameraConnectionState::Idle,
        });
    }

    Ok(out)
}

fn dedupe_cameras(cameras: &mut Vec<CameraFeed>) {
    let mut seen = HashMap::<String, usize>::new();
    let mut deduped = Vec::with_capacity(cameras.len());
    for camera in cameras.drain(..) {
        if seen.contains_key(&camera.id) {
            continue;
        }
        seen.insert(camera.id.clone(), deduped.len());
        deduped.push(camera);
    }
    *cameras = deduped;
}

fn source_signature(sources: &[PublicCameraSource]) -> String {
    sources
        .iter()
        .map(|source| format!("{}@{}", source.name, source.endpoint))
        .collect::<Vec<_>>()
        .join(",")
}

fn scrape_source_signature(sources: &[ScrapedCameraSource]) -> String {
    sources
        .iter()
        .map(|source| format!("{}@{}", source.name, source.page_url))
        .collect::<Vec<_>>()
        .join(",")
}

fn value_as_string(value: &Value) -> Option<String> {
    if let Some(value) = value.as_str() {
        Some(value.to_owned())
    } else if let Some(value) = value.as_i64() {
        Some(value.to_string())
    } else if let Some(value) = value.as_u64() {
        Some(value.to_string())
    } else {
        value.as_f64().map(|value| value.to_string())
    }
}

fn value_as_string_ref(value: &Value) -> Option<String> {
    value_as_string(value)
}

fn extract_scraped_label(source: &ScrapedCameraSource, html: &str) -> Option<String> {
    match source.kind {
        ScrapedCameraSourceKind::GenericHtml
        | ScrapedCameraSourceKind::Opentopia
        | ScrapedCameraSourceKind::Webcamera24
        | ScrapedCameraSourceKind::WorldcamsTv
        | ScrapedCameraSourceKind::SkylineWebcams
        | ScrapedCameraSourceKind::Webcamtaxi => extract_meta_content(html, "og:title")
            .or_else(|| extract_meta_content(html, "twitter:title"))
            .or_else(|| extract_title_text(html)),
    }
    .map(|label| clean_text(&label))
    .filter(|label| !label.is_empty())
}

fn extract_scraped_stream_url(source: &ScrapedCameraSource, html: &str) -> Option<String> {
    match source.kind {
        ScrapedCameraSourceKind::GenericHtml
        | ScrapedCameraSourceKind::Webcamera24
        | ScrapedCameraSourceKind::WorldcamsTv
        | ScrapedCameraSourceKind::SkylineWebcams
        | ScrapedCameraSourceKind::Webcamtaxi => extract_meta_content(html, "og:video")
            .or_else(|| extract_meta_content(html, "og:video:url"))
            .or_else(|| extract_first_tag_attribute(html, "iframe", "src"))
            .or_else(|| extract_first_tag_attribute(html, "source", "src"))
            .or_else(|| extract_first_tag_attribute(html, "video", "src")),
        ScrapedCameraSourceKind::Opentopia => extract_first_tag_attribute(html, "iframe", "src")
            .or_else(|| extract_first_tag_attribute(html, "img", "src"))
            .or_else(|| extract_meta_content(html, "og:image")),
    }
}

fn extract_scraped_point(source: &ScrapedCameraSource, html: &str) -> Option<(f32, f32)> {
    match source.kind {
        ScrapedCameraSourceKind::GenericHtml
        | ScrapedCameraSourceKind::Webcamera24
        | ScrapedCameraSourceKind::WorldcamsTv
        | ScrapedCameraSourceKind::SkylineWebcams
        | ScrapedCameraSourceKind::Webcamtaxi
        | ScrapedCameraSourceKind::Opentopia => extract_coordinate_pair_from_html(html),
    }
}

fn extract_meta_content(html: &str, property: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let property_lower = property.to_ascii_lowercase();
    for attr in ["property", "name"] {
        let marker = format!(r#"{attr}="{property_lower}""#);
        if let Some(index) = lower.find(&marker) {
            let fragment = html_fragment(html, index)?;
            if let Some(content) = extract_attribute_value(fragment, "content") {
                return Some(content);
            }
        }
    }
    None
}

fn extract_title_text(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let start = lower.find("<title")?;
    let open_end = lower[start..].find('>')? + start + 1;
    let end = lower[open_end..].find("</title>")? + open_end;
    Some(html[open_end..end].to_owned())
}

fn extract_first_tag_attribute(html: &str, tag: &str, attr: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let marker = format!("<{}", tag.to_ascii_lowercase());
    let mut search_start = 0;
    while let Some(relative) = lower[search_start..].find(&marker) {
        let index = search_start + relative;
        let fragment = html_fragment(html, index)?;
        if let Some(value) = extract_attribute_value(fragment, attr) {
            return Some(value);
        }
        search_start = index + marker.len();
    }
    None
}

fn html_fragment(html: &str, index: usize) -> Option<&str> {
    let end = html[index..].find('>')? + index + 1;
    Some(&html[index..end])
}

fn extract_attribute_value(fragment: &str, attr: &str) -> Option<String> {
    let lower = fragment.to_ascii_lowercase();
    let attr_lower = attr.to_ascii_lowercase();
    for quote in ['"', '\''] {
        let marker = format!(r#"{attr_lower}={quote}"#);
        if let Some(start) = lower.find(&marker) {
            let value_start = start + marker.len();
            let value_end = fragment[value_start..].find(quote)? + value_start;
            return Some(fragment[value_start..value_end].to_owned());
        }
    }
    None
}

fn clean_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn extract_coordinate_pair_from_html(html: &str) -> Option<(f32, f32)> {
    let patterns = ["@", "ll=", "center=", "q=", "lat=", "latitude="];
    patterns
        .iter()
        .find_map(|pattern| extract_coordinate_pair_after(html, pattern))
}

fn extract_coordinate_pair_after(text: &str, pattern: &str) -> Option<(f32, f32)> {
    let lower = text.to_ascii_lowercase();
    let pattern_lower = pattern.to_ascii_lowercase();
    let mut search_start = 0;
    while let Some(relative) = lower[search_start..].find(&pattern_lower) {
        let start = search_start + relative + pattern_lower.len();
        let tail = &text[start..];
        if let Some((lat, lon, consumed)) = parse_coordinate_pair_prefix(tail) {
            let preceding = lower[..start].chars().rev().take(24).collect::<String>();
            if pattern_lower == "@" || preceding.contains("google") || preceding.contains("map") {
                return Some((lat, lon));
            }
            search_start = start + consumed;
            continue;
        }
        search_start = start;
    }
    None
}

fn parse_coordinate_pair_prefix(text: &str) -> Option<(f32, f32, usize)> {
    let trimmed = text.trim_start_matches(|ch: char| ch.is_whitespace() || ch == '"' || ch == '\'');
    let offset = text.len().saturating_sub(trimmed.len());
    let (lat, lat_len) = parse_leading_float(trimmed)?;
    let remainder = &trimmed[lat_len..];
    let remainder = remainder.strip_prefix(',')?;
    let (lon, lon_len) = parse_leading_float(remainder)?;
    Some((lat, lon, offset + lat_len + 1 + lon_len))
}

fn parse_leading_float(text: &str) -> Option<(f32, usize)> {
    let mut end = 0;
    for (index, ch) in text.char_indices() {
        if ch.is_ascii_digit() || ch == '-' || ch == '+' || ch == '.' {
            end = index + ch.len_utf8();
        } else {
            break;
        }
    }
    if end == 0 {
        return None;
    }
    let value = text[..end].parse::<f32>().ok()?;
    Some((value, end))
}

fn value_as_f32(value: &Value) -> Option<f32> {
    if let Some(number) = value.as_f64() {
        Some(number as f32)
    } else if let Some(number) = value.as_i64() {
        Some(number as f32)
    } else if let Some(text) = value.as_str() {
        text.parse::<f32>().ok()
    } else {
        None
    }
}

fn value_as_f32_ref(value: &Value) -> Option<f32> {
    value_as_f32(value)
}

fn slugify(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last_dash = false;
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    out.trim_matches('-').to_owned()
}

fn current_generation() -> u64 {
    manager().lock().unwrap().generation
}

fn manager() -> &'static Mutex<PollManager> {
    static MANAGER: OnceLock<Mutex<PollManager>> = OnceLock::new();
    MANAGER.get_or_init(|| {
        Mutex::new(PollManager {
            active: None,
            generation: 0,
            tracked_signature: String::new(),
            next_poll_at: Some(Instant::now()),
            shutdown: false,
        })
    })
}
