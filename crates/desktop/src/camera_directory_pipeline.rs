//! Opt-in public camera-directory ingestion inspired by Project Eyes On.
//!
//! The original tool combines directory scraping with broad search-engine
//! dorking. 1kEE intentionally ports only the allowlisted directory pipeline:
//! discover advertised feeds, deduplicate them, read coordinates from the
//! directory's own detail pages, and perform a bounded reachability probe.

use crate::model::{CameraConnectionState, CameraFeed, GeoPoint};
use rayon::prelude::*;
use reqwest::Url;
use reqwest::blocking::Client;
use reqwest::header::{ACCEPT, CONTENT_TYPE, USER_AGENT};
use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const INSECAM_ORIGIN: &str = "http://www.insecam.org";
const DIRECTORY_TIMEOUT: Duration = Duration::from_secs(12);
const DETAIL_TIMEOUT: Duration = Duration::from_secs(10);
const FEED_PROBE_TIMEOUT: Duration = Duration::from_secs(6);
const MAX_CAMERAS_PER_POLL: usize = 60;
const BROWSER_USER_AGENT: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 Chrome/120 Safari/537.36";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EyesOnPipelineConfig {
    pub country_code: Option<String>,
    pub max_pages: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EyesOnPipelineProgress {
    DirectoryPages {
        completed: usize,
        total: usize,
    },
    Candidates {
        found: usize,
    },
    FeedChecks {
        completed: usize,
        total: usize,
        geolocated: usize,
        reachable: usize,
    },
}

#[derive(Clone, Debug)]
struct DirectoryCandidate {
    camera_id: String,
    feed_url: String,
    detail_url: String,
    brand: String,
    location_hint: String,
}

pub fn fetch<F>(
    client: &Client,
    config: &EyesOnPipelineConfig,
    on_progress: F,
) -> Result<Vec<CameraFeed>, String>
where
    F: Fn(EyesOnPipelineProgress) + Sync,
{
    let max_pages = config.max_pages.clamp(1, 5);
    let total_pages = max_pages as usize;
    let completed_pages = AtomicUsize::new(0);
    on_progress(EyesOnPipelineProgress::DirectoryPages {
        completed: 0,
        total: total_pages,
    });
    let page_results: Vec<_> = (1..=max_pages)
        .into_par_iter()
        .map(|page| {
            let result = fetch_directory_page(client, config.country_code.as_deref(), page);
            let completed = completed_pages.fetch_add(1, Ordering::AcqRel) + 1;
            on_progress(EyesOnPipelineProgress::DirectoryPages {
                completed,
                total: total_pages,
            });
            result
        })
        .collect();

    let successful_pages = page_results.iter().filter(|result| result.is_ok()).count();
    if successful_pages == 0 {
        let detail = page_results
            .into_iter()
            .filter_map(Result::err)
            .next()
            .unwrap_or_else(|| "directory returned no readable pages".to_owned());
        return Err(detail);
    }

    let mut seen_urls = HashSet::new();
    let mut candidates = Vec::new();
    for candidate in page_results.into_iter().filter_map(Result::ok).flatten() {
        if seen_urls.insert(candidate.feed_url.clone()) {
            candidates.push(candidate);
        }
        if candidates.len() >= MAX_CAMERAS_PER_POLL {
            break;
        }
    }

    on_progress(EyesOnPipelineProgress::Candidates {
        found: candidates.len(),
    });

    let total_candidates = candidates.len();
    let completed_checks = AtomicUsize::new(0);
    let geolocated = AtomicUsize::new(0);
    let reachable = AtomicUsize::new(0);
    let mut cameras: Vec<_> = candidates
        .into_par_iter()
        .filter_map(|candidate| {
            let camera = enrich_candidate(client, candidate);
            if let Some(camera) = &camera {
                geolocated.fetch_add(1, Ordering::AcqRel);
                if camera.status == CameraConnectionState::Reachable {
                    reachable.fetch_add(1, Ordering::AcqRel);
                }
            }
            let completed = completed_checks.fetch_add(1, Ordering::AcqRel) + 1;
            on_progress(EyesOnPipelineProgress::FeedChecks {
                completed,
                total: total_candidates,
                geolocated: geolocated.load(Ordering::Acquire),
                reachable: reachable.load(Ordering::Acquire),
            });
            camera
        })
        .collect();
    cameras.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(cameras)
}

fn fetch_directory_page(
    client: &Client,
    country_code: Option<&str>,
    page: u8,
) -> Result<Vec<DirectoryCandidate>, String> {
    let url = directory_url(country_code, page);
    let response = client
        .get(&url)
        .header(USER_AGENT, BROWSER_USER_AGENT)
        .header(ACCEPT, "text/html,application/xhtml+xml")
        .timeout(DIRECTORY_TIMEOUT)
        .send()
        .map_err(|error| format!("{url}: {error}"))?;
    if !response.status().is_success() {
        return Err(format!("{url}: unexpected status {}", response.status()));
    }
    let body = response.text().map_err(|error| format!("{url}: {error}"))?;
    Ok(parse_directory_page(&body))
}

fn directory_url(country_code: Option<&str>, page: u8) -> String {
    match country_code.filter(|code| !code.trim().is_empty()) {
        Some(code) => format!(
            "{INSECAM_ORIGIN}/en/bycountry/{}/?page={}",
            code.trim().to_ascii_uppercase(),
            page.max(1)
        ),
        None => format!("{INSECAM_ORIGIN}/en/byrating/?page={}", page.max(1)),
    }
}

fn parse_directory_page(html: &str) -> Vec<DirectoryCandidate> {
    let lower = html.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut search_start = 0;

    while let Some(relative_start) = lower[search_start..].find("<img") {
        let image_start = search_start + relative_start;
        let Some(relative_end) = lower[image_start..].find('>') else {
            break;
        };
        let image_end = image_start + relative_end + 1;
        let image_tag = &html[image_start..image_end];
        search_start = image_end;

        let Some(raw_title) = extract_attribute_value(image_tag, "title") else {
            continue;
        };
        let title = decode_html_attribute(&raw_title);
        let Some((brand, location_hint)) = parse_camera_title(&title) else {
            continue;
        };
        let Some(raw_feed_url) = extract_attribute_value(image_tag, "src") else {
            continue;
        };
        let feed_url = decode_html_attribute(&raw_feed_url);
        if validated_public_feed_url(&feed_url).is_none() {
            continue;
        }

        let context_start = image_start.saturating_sub(1_200);
        let context_lower = &lower[context_start..image_start];
        let Some(anchor_relative) = context_lower.rfind("<a") else {
            continue;
        };
        let anchor_start = context_start + anchor_relative;
        let Some(anchor_relative_end) = lower[anchor_start..].find('>') else {
            continue;
        };
        let anchor_tag = &html[anchor_start..anchor_start + anchor_relative_end + 1];
        let Some(raw_detail_path) = extract_attribute_value(anchor_tag, "href") else {
            continue;
        };
        let Some(detail_url) = directory_detail_url(&decode_html_attribute(&raw_detail_path))
        else {
            continue;
        };
        let Some(camera_id) = camera_id_from_detail_url(&detail_url) else {
            continue;
        };

        out.push(DirectoryCandidate {
            camera_id,
            feed_url,
            detail_url,
            brand,
            location_hint,
        });
    }

    out
}

fn parse_camera_title(title: &str) -> Option<(String, String)> {
    let normalized = title.split_whitespace().collect::<Vec<_>>().join(" ");
    let lower = normalized.to_ascii_lowercase();
    let split_at = lower.find(" in ")?;
    let prefix = normalized[..split_at].trim();
    let location = normalized[split_at + 4..].trim();
    let brand = prefix
        .strip_prefix("Live camera ")
        .or_else(|| prefix.strip_prefix("live camera "))
        .unwrap_or(prefix)
        .trim();
    if brand.is_empty() || location.is_empty() {
        return None;
    }
    Some((brand.to_owned(), location.to_owned()))
}

fn directory_detail_url(path: &str) -> Option<String> {
    let origin = Url::parse(INSECAM_ORIGIN).ok()?;
    let url = origin.join(path).ok()?;
    let host = url.host_str()?.trim_start_matches("www.");
    (host.eq_ignore_ascii_case("insecam.org") && url.path().starts_with("/en/view/"))
        .then(|| url.to_string())
}

fn camera_id_from_detail_url(url: &str) -> Option<String> {
    let url = Url::parse(url).ok()?;
    url.path_segments()?
        .filter(|part| !part.is_empty())
        .next_back()
        .filter(|part| part.bytes().all(|byte| byte.is_ascii_digit()))
        .map(str::to_owned)
}

fn enrich_candidate(client: &Client, candidate: DirectoryCandidate) -> Option<CameraFeed> {
    let location = fetch_directory_point(client, &candidate.detail_url)?;
    let (kind, status) = probe_feed(client, &candidate.feed_url);
    let last_seen = match status {
        CameraConnectionState::Reachable => "verified during Eyes On sync",
        CameraConnectionState::Unreachable => "directory listed; feed probe failed",
        CameraConnectionState::Attempted => "directory listed; feed type unconfirmed",
        CameraConnectionState::Idle => "directory listed",
    };

    Some(CameraFeed {
        id: format!("eyes-on-{}", candidate.camera_id),
        label: format!("{} · {}", candidate.brand, candidate.location_hint),
        provider: "Project Eyes On · Insecam".into(),
        kind,
        location,
        stream_url: candidate.feed_url,
        last_seen: last_seen.into(),
        status,
    })
}

fn fetch_directory_point(client: &Client, detail_url: &str) -> Option<GeoPoint> {
    let response = client
        .get(detail_url)
        .header(USER_AGENT, BROWSER_USER_AGENT)
        .header(ACCEPT, "text/html,application/xhtml+xml")
        .timeout(DETAIL_TIMEOUT)
        .send()
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    extract_detail_point(&response.text().ok()?)
}

fn extract_detail_point(html: &str) -> Option<GeoPoint> {
    let lower = html.to_ascii_lowercase();
    for marker in ["setview([", "l.marker(["] {
        let Some(start) = lower.find(marker) else {
            continue;
        };
        let tail = &html[start + marker.len()..];
        if let Some((lat, lon)) = parse_coordinate_pair_prefix(tail) {
            if (-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lon) {
                return Some(GeoPoint { lat, lon });
            }
        }
    }
    None
}

fn parse_coordinate_pair_prefix(text: &str) -> Option<(f32, f32)> {
    let trimmed = text.trim_start();
    let comma = trimmed.find(',')?;
    let lat = trimmed[..comma].trim().parse::<f32>().ok()?;
    let remainder = trimmed[comma + 1..].trim_start();
    let lon_end = remainder
        .find(|character: char| {
            !(character.is_ascii_digit() || matches!(character, '-' | '+' | '.'))
        })
        .unwrap_or(remainder.len());
    let lon = remainder[..lon_end].parse::<f32>().ok()?;
    Some((lat, lon))
}

fn probe_feed(client: &Client, advertised_url: &str) -> (String, CameraConnectionState) {
    let url = materialize_feed_url(advertised_url);
    let response = client
        .get(&url)
        .header(USER_AGENT, BROWSER_USER_AGENT)
        .header(ACCEPT, "image/avif,image/webp,image/*,video/*,*/*;q=0.8")
        .timeout(FEED_PROBE_TIMEOUT)
        .send();

    let Ok(response) = response else {
        return (
            kind_from_url(&url).into(),
            CameraConnectionState::Unreachable,
        );
    };
    if !response.status().is_success() {
        return (
            kind_from_url(&url).into(),
            CameraConnectionState::Unreachable,
        );
    }

    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let kind = classify_feed(content_type, &url);
    let status = if kind == "camera" {
        CameraConnectionState::Attempted
    } else {
        CameraConnectionState::Reachable
    };
    (kind.into(), status)
}

fn classify_feed(content_type: &str, url: &str) -> &'static str {
    let content_type = content_type.to_ascii_lowercase();
    if content_type.contains("multipart") || content_type.contains("x-mixed-replace") {
        "mjpeg stream"
    } else if content_type.starts_with("image/") {
        "snapshot"
    } else if content_type.starts_with("video/") {
        "video feed"
    } else {
        kind_from_url(url)
    }
}

fn kind_from_url(url: &str) -> &'static str {
    let lower = url.to_ascii_lowercase();
    if lower.contains("mjpg") || lower.contains("mjpeg") || lower.contains("faststream") {
        "mjpeg stream"
    } else if lower.contains("snapshot")
        || lower.contains("jpeg")
        || lower.contains(".jpg")
        || lower.contains("/camera")
    {
        "snapshot"
    } else if lower.contains("video") {
        "video feed"
    } else {
        "camera"
    }
}

fn validated_public_feed_url(raw_url: &str) -> Option<Url> {
    let url = Url::parse(raw_url).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    let host = url.host_str()?;
    let ip = host.parse::<IpAddr>().ok()?;
    is_public_ip(ip).then_some(url)
}

fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_public_ipv4(ip),
        IpAddr::V6(ip) => is_public_ipv6(ip),
    }
}

fn is_public_ipv4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    if matches!(a, 0 | 10 | 127 | 224..=255)
        || (a == 100 && (64..=127).contains(&b))
        || (a == 169 && b == 254)
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && b == 168)
        || (a == 198 && matches!(b, 18 | 19))
    {
        return false;
    }
    !matches!(
        (a, b, c),
        (192, 0, 0) | (192, 0, 2) | (192, 88, 99) | (198, 51, 100) | (203, 0, 113)
    )
}

fn is_public_ipv6(ip: Ipv6Addr) -> bool {
    let segments = ip.segments();
    !ip.is_unspecified()
        && !ip.is_loopback()
        && !ip.is_multicast()
        && (segments[0] & 0xfe00) != 0xfc00
        && (segments[0] & 0xffc0) != 0xfe80
        && !(segments[0] == 0x2001 && segments[1] == 0x0db8)
}

fn materialize_feed_url(url: &str) -> String {
    let counter = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    url.replace("COUNTER", &counter.to_string())
}

fn extract_attribute_value(fragment: &str, attribute: &str) -> Option<String> {
    let lower = fragment.to_ascii_lowercase();
    let attribute = attribute.to_ascii_lowercase();
    for quote in ['"', '\''] {
        let marker = format!("{attribute}={quote}");
        if let Some(start) = lower.find(&marker) {
            let value_start = start + marker.len();
            let value_end = fragment[value_start..].find(quote)? + value_start;
            return Some(fragment[value_start..value_end].to_owned());
        }
    }
    None
}

fn decode_html_attribute(value: &str) -> String {
    let mut decoded = value.trim().to_owned();
    for _ in 0..3 {
        let next = decoded
            .replace("&amp;", "&")
            .replace("&quot;", "\"")
            .replace("&#39;", "'")
            .replace("&lt;", "<")
            .replace("&gt;", ">");
        if next == decoded {
            break;
        }
        decoded = next;
    }
    decoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directory_urls_are_bounded_and_country_scoped() {
        assert_eq!(
            directory_url(None, 0),
            "http://www.insecam.org/en/byrating/?page=1"
        );
        assert_eq!(
            directory_url(Some(" us "), 3),
            "http://www.insecam.org/en/bycountry/US/?page=3"
        );
    }

    #[test]
    fn directory_parser_extracts_and_deduplicates_useful_metadata() {
        let html = r#"
          <a class="thumbnail-item__wrap" href="/en/view/521291/">
            <img src="http://202.245.13.81:80/cgi-bin/camera?quality=1&amp;amp;COUNTER"
                 title="Live camera PanasonicHD in Tokyo, Japan" />
          </a>
          <img src="/static/logo.png" title="Insecam" />
          <a href="/en/view/964631/">
            <img src="http://190.210.250.149:91/mjpg/video.mjpg"
                 title="Live camera Axis in Buenos Aires, Argentina" />
          </a>
        "#;

        let parsed = parse_directory_page(html);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].camera_id, "521291");
        assert_eq!(parsed[0].brand, "PanasonicHD");
        assert_eq!(parsed[0].location_hint, "Tokyo, Japan");
        assert_eq!(
            parsed[0].feed_url,
            "http://202.245.13.81:80/cgi-bin/camera?quality=1&COUNTER"
        );
        assert_eq!(parsed[1].camera_id, "964631");
    }

    #[test]
    fn detail_parser_reads_directory_coordinates() {
        let html = "const map = L.map('map').setView([35.689506, 139.691700], 9);";
        assert_eq!(
            extract_detail_point(html),
            Some(GeoPoint {
                lat: 35.689506,
                lon: 139.6917,
            })
        );
        assert_eq!(extract_detail_point("setView([999, 1], 9)"), None);
    }

    #[test]
    fn feed_validation_rejects_internal_or_credentialed_targets() {
        assert!(validated_public_feed_url("http://8.8.8.8/camera.jpg").is_some());
        assert!(validated_public_feed_url("http://127.0.0.1/camera.jpg").is_none());
        assert!(validated_public_feed_url("http://192.168.1.5/camera.jpg").is_none());
        assert!(validated_public_feed_url("http://user:pass@8.8.8.8/camera.jpg").is_none());
        assert!(validated_public_feed_url("file:///tmp/camera.jpg").is_none());
    }

    #[test]
    fn feed_classification_prefers_response_content_type() {
        assert_eq!(
            classify_feed(
                "multipart/x-mixed-replace; boundary=frame",
                "http://8.8.8.8/"
            ),
            "mjpeg stream"
        );
        assert_eq!(
            classify_feed("image/jpeg", "http://8.8.8.8/video"),
            "snapshot"
        );
        assert_eq!(
            classify_feed("text/plain", "http://8.8.8.8/video.mjpg"),
            "mjpeg stream"
        );
    }
}
