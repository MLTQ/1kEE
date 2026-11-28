use crate::camera_directory_pipeline::{self, EyesOnPipelineConfig, EyesOnPipelineProgress};
use crate::camera_scrape_catalog::{self, ScrapedCameraSource, ScrapedCameraSourceKind};
use crate::camera_source_catalog::{self, PublicCameraSource, PublicCameraSourceKind};
use crate::model::{AppModel, CameraConnectionState, CameraFeed, GeoPoint};
use crate::settings_store;
use crossbeam_channel::{Receiver, Sender};
use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, HeaderValue};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
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
    progress: Receiver<PollProgress>,
    cancel: Arc<AtomicBool>,
}

#[derive(Clone, Debug, PartialEq)]
struct PollProgress {
    fraction: f32,
    label: String,
}

enum PollOutcome {
    Success {
        cameras: Vec<CameraFeed>,
        source_label: String,
        warnings: Vec<String>,
    },
    Error(String),
}

pub fn tick(model: &mut AppModel) {
    let signature = poll_signature(model);
    let now = Instant::now();
    let mut finished = None;
    let mut progress_update = None;

    {
        let mut manager = manager().lock().unwrap();
        if manager.tracked_signature != signature {
            if let Some(active) = manager.active.as_ref() {
                active.cancel.store(true, Ordering::Release);
            }
            manager.tracked_signature = signature.clone();
            manager.generation = manager.generation.wrapping_add(1);
            manager.next_poll_at = Some(now);
            manager.shutdown = false;
        }

        if let Some(active) = manager.active.as_ref() {
            let newest_progress = active.progress.try_iter().last();
            if active.generation == manager.generation {
                progress_update = newest_progress;
            }
            if active.handle.is_finished() {
                finished = manager
                    .active
                    .take()
                    .map(|active| (active.generation, active.handle));
            }
        }
    }

    if let Some(progress) = progress_update {
        model.camera_registry_scanning = true;
        model.camera_registry_progress = progress.fraction.clamp(0.0, 1.0);
        model.camera_registry_progress_label = progress.label;
    }

    if let Some((generation, handle)) = finished {
        match handle.join() {
            Ok(outcome) => apply_outcome(model, generation, outcome),
            Err(_) => {
                if generation == current_generation() {
                    model.camera_registry_status = "error".into();
                    clear_progress(model);
                    model.push_log("Camera registry worker panicked before returning data.".into());
                }
            }
        }
    }

    let public_sources = camera_source_catalog::load_public_sources_with_builtin(
        model.selected_root.as_deref(),
        model.open_public_cameras_enabled,
    );
    let scrape_sources = camera_scrape_catalog::load_scrape_sources(model.selected_root.as_deref());
    if !model.has_enabled_camera_sources() && public_sources.is_empty() && scrape_sources.is_empty()
    {
        if model.camera_registry_status != "inactive" {
            model.camera_registry_status = "inactive".into();
        }
        clear_progress(model);
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
        let eyes_on_config = model.eyes_on_enabled.then(|| EyesOnPipelineConfig {
            country_code: (!model.eyes_on_country_code.trim().is_empty())
                .then(|| model.eyes_on_country_code.trim().to_owned()),
            requests_per_minute: settings_store::normalize_eyes_on_requests_per_minute(
                model.eyes_on_requests_per_minute,
            ),
        });
        let windy_centers = collect_windy_centers(model);
        let public_sources = public_sources;
        let scrape_sources = scrape_sources;
        let (progress_sender, progress_receiver) = crossbeam_channel::unbounded();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);

        let handle = thread::spawn(move || {
            fetch_camera_registry(
                &windy_key,
                &ny511_key,
                eyes_on_config.as_ref(),
                &windy_centers,
                &public_sources,
                &scrape_sources,
                &progress_sender,
                &worker_cancel,
            )
        });

        let mut manager = manager().lock().unwrap();
        if !manager.shutdown && manager.generation == spawn_generation {
            manager.active = Some(ActivePoll {
                generation: spawn_generation,
                handle,
                progress: progress_receiver,
                cancel,
            });
            model.camera_registry_status = "syncing".into();
            model.camera_registry_scanning = true;
            model.camera_registry_progress = 0.01;
            model.camera_registry_progress_label = "Loading camera sources…".into();
        }
    }
}

pub fn invalidate() {
    let now = Instant::now();
    let mut manager = manager().lock().unwrap();
    if let Some(active) = manager.active.as_ref() {
        active.cancel.store(true, Ordering::Release);
    }
    manager.generation = manager.generation.wrapping_add(1);
    manager.next_poll_at = Some(now);
    manager.shutdown = false;
}

pub fn shutdown() {
    let mut manager = manager().lock().unwrap();
    if let Some(active) = manager.active.as_ref() {
        active.cancel.store(true, Ordering::Release);
    }
    manager.generation = manager.generation.wrapping_add(1);
    manager.shutdown = true;
    manager.next_poll_at = None;
}

fn apply_outcome(model: &mut AppModel, generation: u64, outcome: PollOutcome) {
    if generation != current_generation() {
        return;
    }

    clear_progress(model);
    match outcome {
        PollOutcome::Success {
            cameras,
            source_label,
            warnings,
        } => {
            // An intentionally emptied endpoint cache must clear old markers,
            // including when the settings window closed before its worker ended.
            model.replace_camera_registry(cameras, &source_label);
            for warning in warnings {
                model.push_log(format!("Camera source warning: {warning}"));
            }
        }
        PollOutcome::Error(error) => {
            model.camera_registry_status = "error".into();
            model.push_log(format!("Camera registry sync failed: {error}"));
        }
    }
}

fn clear_progress(model: &mut AppModel) {
    model.camera_registry_scanning = false;
    model.camera_registry_progress = 0.0;
    model.camera_registry_progress_label.clear();
}

fn report_progress(progress: &Sender<PollProgress>, fraction: f32, label: impl Into<String>) {
    let _ = progress.send(PollProgress {
        fraction: fraction.clamp(0.0, 1.0),
        label: label.into(),
    });
}

fn eyes_on_poll_progress(scope: &str, progress: EyesOnPipelineProgress) -> PollProgress {
    match progress {
        EyesOnPipelineProgress::SavedEndpoints { count } => PollProgress {
            fraction: 0.95,
            label: format!("Loaded {count} saved {scope} camera endpoint(s)"),
        },
        EyesOnPipelineProgress::DirectoryPages {
            completed,
            total,
            candidates,
        } => {
            let fraction = total
                .map(|total| 0.15 + 0.25 * completed as f32 / total.max(1) as f32)
                .unwrap_or_else(|| 0.15 + 0.25 * (1.0 - (-(completed as f32) / 25.0).exp()));
            let page_label = total
                .map(|total| format!("{completed}/{total}"))
                .unwrap_or_else(|| completed.to_string());
            PollProgress {
                fraction,
                label: format!(
                    "Scanning {scope} camera directory · page {page_label} · {candidates} candidates"
                ),
            }
        }
        EyesOnPipelineProgress::DirectoryCache {
            candidates,
            age_seconds,
        } => PollProgress {
            fraction: 0.40,
            label: format!(
                "Reusing {scope} directory snapshot · {candidates} candidates · {} min old",
                age_seconds / 60
            ),
        },
        EyesOnPipelineProgress::Candidates { found, reused } => PollProgress {
            fraction: 0.42,
            label: format!("Found {found} camera listing(s) · reusing {reused} cached result(s)…"),
        },
        EyesOnPipelineProgress::FeedChecks {
            completed,
            total,
            geolocated,
            reachable,
            reused,
        } => {
            let denominator = total.max(1) as f32;
            PollProgress {
                fraction: 0.42 + 0.53 * completed as f32 / denominator,
                label: format!(
                    "Checking cameras {completed}/{total} · {geolocated} geolocated · {reachable} reachable · {reused} cached"
                ),
            }
        }
    }
}

fn collect_windy_centers(model: &AppModel) -> Vec<GeoPoint> {
    let mut centers: Vec<GeoPoint> = Vec::new();
    if let Some(focus) = model.terrain_focus_location() {
        centers.push(focus);
    }
    for cluster in model.convergence.clusters().iter().take(6) {
        if centers.len() >= 3 {
            break;
        }
        let loc = cluster.center;
        if centers
            .iter()
            .all(|c| (c.lat - loc.lat).abs() > 2.5 || (c.lon - loc.lon).abs() > 2.5)
        {
            centers.push(loc);
        }
    }
    centers
}

fn poll_signature(model: &AppModel) -> String {
    let windy_enabled = !model.windy_webcams_api_key.trim().is_empty();
    // Only tie poll signature to focus (quantized to 1°) when Windy is enabled,
    // so panning or switching events never cancels healthy DOT/public/Eyes-On syncs.
    let focus = if windy_enabled {
        model
            .terrain_focus_location()
            .map(|point| format!("{:.0}:{:.0}", point.lat, point.lon))
            .unwrap_or_else(|| "nofocus".into())
    } else {
        "static".into()
    };
    let public_sources = camera_source_catalog::load_public_sources_with_builtin(
        model.selected_root.as_deref(),
        model.open_public_cameras_enabled,
    );
    let scrape_sources = camera_scrape_catalog::load_scrape_sources(model.selected_root.as_deref());
    format!(
        "windy:{}|511ny:{}|open-public:{}|eyes-on:{}:{}:{}rpm|public:{}|scrape:{}|focus:{}",
        windy_enabled,
        !model.ny511_api_key.trim().is_empty(),
        model.open_public_cameras_enabled,
        model.eyes_on_enabled,
        model.eyes_on_country_code.trim(),
        settings_store::normalize_eyes_on_requests_per_minute(model.eyes_on_requests_per_minute),
        source_signature(&public_sources),
        scrape_source_signature(&scrape_sources),
        focus
    )
}

#[allow(clippy::too_many_arguments)]
fn fetch_camera_registry(
    windy_key: &str,
    ny511_key: &str,
    eyes_on_config: Option<&EyesOnPipelineConfig>,
    windy_centers: &[GeoPoint],
    public_sources: &[PublicCameraSource],
    scrape_sources: &[ScrapedCameraSource],
    progress: &Sender<PollProgress>,
    cancelled: &AtomicBool,
) -> PollOutcome {
    report_progress(progress, 0.02, "Preparing camera sources…");
    let client = match Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .user_agent("1kEE/0.1 public-camera-registry")
        .build()
    {
        Ok(client) => client,
        Err(error) => return PollOutcome::Error(error.to_string()),
    };

    let mut cameras = Vec::new();
    let mut source_parts = Vec::new();
    let mut warnings = Vec::new();
    let mut succeeded_sources = 0usize;

    if !ny511_key.trim().is_empty() && !cancelled.load(Ordering::Acquire) {
        report_progress(progress, 0.08, "Querying 511NY cameras…");
        match fetch_511ny_cameras(&client, ny511_key.trim()) {
            Ok(mut fetched) => {
                succeeded_sources += 1;
                source_parts.push("511NY".to_owned());
                cameras.append(&mut fetched);
            }
            Err(error) => {
                warnings.push(format!("511NY adapter failed: {error}"));
            }
        }
    }

    if !windy_key.trim().is_empty() && !windy_centers.is_empty() && !cancelled.load(Ordering::Acquire)
    {
        report_progress(progress, 0.12, "Querying Windy Webcams…");
        let mut windy_ok = false;
        for &center in windy_centers {
            if cancelled.load(Ordering::Acquire) {
                break;
            }
            match fetch_windy_cameras(&client, windy_key.trim(), center) {
                Ok(mut fetched) => {
                    windy_ok = true;
                    cameras.append(&mut fetched);
                }
                Err(error) => {
                    warnings.push(format!("Windy adapter failed: {error}"));
                }
            }
        }
        if windy_ok {
            succeeded_sources += 1;
            source_parts.push("Windy Webcams".to_owned());
        }
    }

    if let Some(config) = eyes_on_config
        && !cancelled.load(Ordering::Acquire)
    {
        let scope = config
            .country_code
            .as_deref()
            .filter(|scope| !scope.trim().is_empty())
            .unwrap_or("global")
            .to_owned();
        match camera_directory_pipeline::fetch(&client, config, cancelled, |update| {
            let update = eyes_on_poll_progress(&scope, update);
            let _ = progress.send(update);
        }) {
            Ok(mut fetched) => {
                succeeded_sources += 1;
                source_parts.push("Project Eyes On · Insecam".to_owned());
                cameras.append(&mut fetched);
            }
            Err(error) => {
                warnings.push(format!("Project Eyes On directory adapter failed: {error}"));
            }
        }
    }

    let public_total = public_sources.len().max(1);
    for (idx, source) in public_sources.iter().enumerate() {
        if cancelled.load(Ordering::Acquire) {
            break;
        }
        report_progress(
            progress,
            0.55 + 0.35 * (idx as f32 / public_total as f32),
            format!("Querying {}…", source.name),
        );
        match fetch_public_source(&client, source) {
            Ok(mut fetched) => {
                succeeded_sources += 1;
                if !fetched.is_empty() {
                    source_parts.push(source.name.clone());
                    cameras.append(&mut fetched);
                }
            }
            Err(error) => {
                warnings.push(format!("Public source '{}' failed: {error}", source.name));
            }
        }
    }

    for source in scrape_sources {
        if cancelled.load(Ordering::Acquire) {
            break;
        }
        match fetch_scraped_source(&client, source) {
            Ok(mut fetched) => {
                succeeded_sources += 1;
                if !fetched.is_empty() {
                    source_parts.push(source.name.clone());
                    cameras.append(&mut fetched);
                }
            }
            Err(error) => {
                warnings.push(format!("Scraped source '{}' failed: {error}", source.name));
            }
        }
    }

    if succeeded_sources == 0 && !warnings.is_empty() {
        return PollOutcome::Error(warnings.join("; "));
    }

    dedupe_cameras(&mut cameras);
    report_progress(
        progress,
        0.98,
        format!("Finalizing {} geolocated camera(s)…", cameras.len()),
    );

    PollOutcome::Success {
        cameras,
        source_label: if source_parts.is_empty() {
            "configured camera sources".into()
        } else {
            source_parts.join(" + ")
        },
        warnings,
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
        PublicCameraSourceKind::TflJamCam => fetch_tfl_jamcam_source(client, source),
        PublicCameraSourceKind::StaticList => Ok(fetch_static_list_source(source)),
    }
}

fn fetch_static_list_source(source: &PublicCameraSource) -> Vec<CameraFeed> {
    source
        .static_cameras
        .iter()
        .filter(|entry| is_safe_public_http_url(&entry.stream_url))
        .map(|entry| CameraFeed {
            id: format!("{}-{}", slugify(&source.provider), slugify(&entry.id)),
            label: entry.label.clone(),
            provider: source.provider.clone(),
            kind: entry
                .kind
                .clone()
                .or_else(|| source.kind_value.clone())
                .unwrap_or_else(|| "snapshot".into()),
            location: GeoPoint {
                lat: entry.latitude,
                lon: entry.longitude,
            },
            stream_url: entry.stream_url.clone(),
            last_seen: "public catalog".into(),
            status: CameraConnectionState::Idle,
        })
        .collect()
}

fn fetch_tfl_jamcam_source(
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
    parse_tfl_jamcam_json(source, &body)
}

fn parse_tfl_jamcam_json(
    source: &PublicCameraSource,
    body: &str,
) -> Result<Vec<CameraFeed>, String> {
    let payload: Value = serde_json::from_str(body).map_err(|error| error.to_string())?;
    let places = payload
        .as_array()
        .ok_or_else(|| "TfL JamCam response was not an array".to_owned())?;
    let mut out = Vec::with_capacity(places.len());
    for place in places {
        let Some(id) = place.get("id").and_then(value_as_string_ref) else {
            continue;
        };
        let Some(lat) = place.get("lat").and_then(value_as_f32_ref) else {
            continue;
        };
        let Some(lon) = place.get("lon").and_then(value_as_f32_ref) else {
            continue;
        };
        let label = place
            .get("commonName")
            .and_then(value_as_string_ref)
            .unwrap_or_else(|| source.name.clone());
        let mut image_url = String::new();
        let mut available = true;
        if let Some(props) = place.get("additionalProperties").and_then(Value::as_array) {
            for prop in props {
                let key = prop.get("key").and_then(Value::as_str).unwrap_or_default();
                let val = prop.get("value").and_then(Value::as_str).unwrap_or_default();
                if key.eq_ignore_ascii_case("imageUrl") && !val.is_empty() {
                    image_url = val.to_owned();
                } else if key.eq_ignore_ascii_case("available") && val.eq_ignore_ascii_case("false")
                {
                    available = false;
                }
            }
        }
        if !available || !is_safe_public_http_url(&image_url) {
            continue;
        }
        out.push(CameraFeed {
            id: format!("{}-{}", slugify(&source.provider), slugify(&id)),
            label,
            provider: source.provider.clone(),
            kind: source.kind_value.clone().unwrap_or_else(|| "snapshot".into()),
            location: GeoPoint { lat, lon },
            stream_url: image_url,
            last_seen: "public source".into(),
            status: CameraConnectionState::Idle,
        });
    }
    Ok(out)
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
    const PAGE_SIZE: usize = 1000;
    const MAX_PAGES: usize = 10;
    let mut out = Vec::new();
    let mut offset = 0usize;

    for _ in 0..MAX_PAGES {
        let query_url = arcgis_query_url(&source.endpoint, offset, PAGE_SIZE);
        let response = client
            .get(&query_url)
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
        let page_count = features.len();

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
                        .and_then(|field| json_path(attributes, field))
                        .and_then(value_as_f32_ref)
                });
            let lon = geometry
                .and_then(|geometry| geometry.get("x"))
                .and_then(value_as_f32_ref)
                .or_else(|| {
                    source
                        .geometry_x_field
                        .as_deref()
                        .and_then(|field| json_path(attributes, field))
                        .and_then(value_as_f32_ref)
                });

            if let Some(camera) = camera_from_record(source, attributes, lat, lon) {
                out.push(camera);
            }
        }

        let exceeded = payload
            .get("exceededTransferLimit")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if !exceeded || page_count == 0 {
            break;
        }
        offset += page_count;
    }
    Ok(out)
}

fn arcgis_query_url(endpoint: &str, offset: usize, page_size: usize) -> String {
    let sep = if endpoint.contains('?') { '&' } else { '?' };
    format!(
        "{endpoint}{sep}where=1%3D1&outFields=*&returnGeometry=true&outSR=4326&resultOffset={offset}&resultRecordCount={page_size}&f=json"
    )
}

fn json_path<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    if !path.contains('.') {
        return value.get(path);
    }
    let mut cur = value;
    for segment in path.split('.') {
        if let Ok(index) = segment.parse::<usize>()
            && let Some(arr) = cur.as_array()
        {
            cur = arr.get(index)?;
        } else {
            cur = cur.get(segment)?;
        }
    }
    Some(cur)
}

fn is_safe_public_http_url(raw: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(raw.trim()) else {
        return false;
    };
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    if host.eq_ignore_ascii_case("localhost") || host.ends_with(".local") {
        return false;
    }
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return match ip {
            std::net::IpAddr::V4(v4) => {
                !(v4.is_private()
                    || v4.is_loopback()
                    || v4.is_link_local()
                    || v4.is_unspecified()
                    || v4.is_multicast())
            }
            std::net::IpAddr::V6(v6) => {
                !(v6.is_loopback() || v6.is_unspecified() || v6.is_multicast())
            }
        };
    }
    true
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

    let id = json_path(record, id_field).and_then(value_as_string_ref)?;
    let label = json_path(record, label_field)
        .and_then(value_as_string_ref)
        .unwrap_or_else(|| source.name.clone());
    let lat = lat_override.or_else(|| json_path(record, lat_field).and_then(value_as_f32_ref))?;
    let lon = lon_override.or_else(|| json_path(record, lon_field).and_then(value_as_f32_ref))?;
    if !lat.is_finite() || !lon.is_finite() || lat.abs() > 90.0 || lon.abs() > 180.0 {
        return None;
    }
    let stream_url = source
        .stream_url_field
        .as_deref()
        .and_then(|field| json_path(record, field))
        .and_then(value_as_string_ref)
        .filter(|u| !u.trim().is_empty())
        .or_else(|| {
            source
                .stream_url_template
                .as_deref()
                .map(|tpl| tpl.replace("{id}", &id))
        })
        .unwrap_or_default();
    if !stream_url.is_empty() && !is_safe_public_http_url(&stream_url) {
        return None;
    }

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
    const PAGE_LIMIT: usize = 50;
    const MAX_PAGES: usize = 4;
    let west = (focus.lon - WINDY_BBOX_HALF_SPAN_DEG).clamp(-180.0, 180.0);
    let east = (focus.lon + WINDY_BBOX_HALF_SPAN_DEG).clamp(-180.0, 180.0);
    let south = (focus.lat - WINDY_BBOX_HALF_SPAN_DEG).clamp(-85.0, 85.0);
    let north = (focus.lat + WINDY_BBOX_HALF_SPAN_DEG).clamp(-85.0, 85.0);

    let mut headers = HeaderMap::new();
    headers.insert(
        "x-windy-api-key",
        HeaderValue::from_str(api_key).map_err(|error| error.to_string())?,
    );

    let mut out = Vec::new();
    for page in 0..MAX_PAGES {
        let offset = page * PAGE_LIMIT;
        let url = format!(
            "https://api.windy.com/webcams/api/v3/webcams?bbox={west},{south},{east},{north}&limit={PAGE_LIMIT}&offset={offset}&include=location,urls,images,player"
        );

        let response = client
            .get(url)
            .headers(headers.clone())
            .send()
            .map_err(|error| error.to_string())?;
        if !response.status().is_success() {
            if page > 0 {
                break;
            }
            return Err(format!("unexpected status {}", response.status()));
        }

        let body = response.text().map_err(|error| error.to_string())?;
        let batch = parse_windy_cameras_json(&body)?;
        let count = batch.len();
        out.extend(batch);
        if count < PAGE_LIMIT {
            break;
        }
    }

    Ok(out)
}

fn parse_windy_cameras_json(body: &str) -> Result<Vec<CameraFeed>, String> {
    let payload: Value = serde_json::from_str(body).map_err(|error| error.to_string())?;
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
        let Some(id) = raw
            .get("webcamId")
            .or_else(|| raw.get("id"))
            .and_then(value_as_string)
        else {
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

        // Prefer direct JPEG preview image URLs that CameraFeedPlayer can render directly.
        let direct_image = json_path(raw, "images.current.preview")
            .or_else(|| json_path(raw, "images.daylight.preview"))
            .or_else(|| json_path(raw, "images.current.thumbnail"))
            .and_then(value_as_string);
        let (stream_url, kind) = if let Some(img) = direct_image {
            (img, "snapshot")
        } else {
            let fallback = raw
                .get("player")
                .and_then(|player| player.get("live"))
                .and_then(value_as_string)
                .or_else(|| {
                    raw.get("urls")
                        .and_then(|urls| urls.get("detail"))
                        .and_then(value_as_string)
                })
                .unwrap_or_default();
            (fallback, "webcam")
        };

        out.push(CameraFeed {
            id: format!("windy-{id}"),
            label,
            provider: "Windy Webcams".into(),
            kind: kind.into(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eyes_on_directory_progress_includes_page_counts() {
        let progress = eyes_on_poll_progress(
            "US",
            EyesOnPipelineProgress::DirectoryPages {
                completed: 3,
                total: Some(30),
                candidates: 18,
            },
        );

        assert!((progress.fraction - 0.175).abs() < f32::EPSILON);
        assert_eq!(
            progress.label,
            "Scanning US camera directory · page 3/30 · 18 candidates"
        );
    }

    #[test]
    fn eyes_on_feed_progress_explains_filtered_results() {
        let progress = eyes_on_poll_progress(
            "global",
            EyesOnPipelineProgress::FeedChecks {
                completed: 4,
                total: 10,
                geolocated: 2,
                reachable: 1,
                reused: 3,
            },
        );

        assert!((progress.fraction - 0.632).abs() < 0.000_1);
        assert_eq!(
            progress.label,
            "Checking cameras 4/10 · 2 geolocated · 1 reachable · 3 cached"
        );
    }

    #[test]
    fn eyes_on_cached_directory_progress_reports_snapshot_age() {
        let progress = eyes_on_poll_progress(
            "global",
            EyesOnPipelineProgress::DirectoryCache {
                candidates: 512,
                age_seconds: 754,
            },
        );

        assert_eq!(progress.fraction, 0.40);
        assert_eq!(
            progress.label,
            "Reusing global directory snapshot · 512 candidates · 12 min old"
        );
    }

    #[test]
    fn windy_v3_parser_prefers_direct_preview_image_over_html_player() {
        let json = r#"{
            "webcams": [{
                "webcamId": 12345,
                "title": "Golden Gate Bridge",
                "location": { "latitude": 37.8199, "longitude": -122.4783 },
                "images": { "current": { "preview": "https://images-webcams.windy.com/45/12345/current/preview/12345.jpg" } },
                "player": { "live": "https://webcams.windy.com/webcams/public/embed/player/12345/live" }
            }]
        }"#;
        let cams = parse_windy_cameras_json(json).unwrap();
        assert_eq!(cams.len(), 1);
        assert_eq!(cams[0].id, "windy-12345");
        assert_eq!(cams[0].kind, "snapshot");
        assert!(cams[0].stream_url.ends_with(".jpg"));
    }

    #[test]
    fn dot_path_and_template_extraction_handles_caltrans_and_digitraffic() {
        let caltrans = &camera_source_catalog::builtin_public_sources()[0];
        let record: Value = serde_json::json!({
            "cctv": {
                "index": "101",
                "location": {
                    "locationName": "US-101 at Golden Gate",
                    "latitude": "37.8199",
                    "longitude": "-122.4783"
                },
                "imageData": {
                    "static": { "currentImageURL": "https://cwwp2.dot.ca.gov/data/d4/cctv/image/tv101.jpg" }
                }
            }
        });
        let cam = camera_from_record(caltrans, &record, None, None).unwrap();
        assert_eq!(cam.label, "US-101 at Golden Gate");
        assert_eq!(cam.stream_url, "https://cwwp2.dot.ca.gov/data/d4/cctv/image/tv101.jpg");

        let arcgis_url = arcgis_query_url("https://services.arcgis.com/Layer/query", 1000, 1000);
        assert!(arcgis_url.contains("outSR=4326"));
        assert!(arcgis_url.contains("resultOffset=1000"));
    }

    #[test]
    fn tfl_jamcam_parser_extracts_available_cameras_only() {
        let source = camera_source_catalog::builtin_public_sources()
            .into_iter()
            .find(|s| s.kind == PublicCameraSourceKind::TflJamCam)
            .unwrap();
        let body = r#"[
            {
                "id": "JamCams_00001.01234",
                "commonName": "Tower Bridge Rd",
                "lat": 51.5055,
                "lon": -0.0754,
                "additionalProperties": [
                    { "key": "available", "value": "true" },
                    { "key": "imageUrl", "value": "https://s3-eu-west-1.amazonaws.com/jamcams.tfl.gov.uk/00001.01234.jpg" }
                ]
            },
            {
                "id": "JamCams_00001.09999",
                "commonName": "Offline Cam",
                "lat": 51.51,
                "lon": -0.08,
                "additionalProperties": [
                    { "key": "available", "value": "false" },
                    { "key": "imageUrl", "value": "https://s3-eu-west-1.amazonaws.com/jamcams.tfl.gov.uk/00001.09999.jpg" }
                ]
            }
        ]"#;
        let cams = parse_tfl_jamcam_json(&source, body).unwrap();
        assert_eq!(cams.len(), 1);
        assert_eq!(cams[0].label, "Tower Bridge Rd");
        assert!(cams[0].stream_url.ends_with("00001.01234.jpg"));
    }
}
