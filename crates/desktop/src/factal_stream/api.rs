use super::parse::parse_event;
use crate::model::EventRecord;
use reqwest::blocking::Client;
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use reqwest::{Url, redirect};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashSet;
use std::thread;
use std::time::Duration;

const BASE_URL: &str = "https://www.factal.com/api/v2/item/";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const PAGE_DELAY: Duration = Duration::from_millis(600);
pub(super) const LIVE_WINDOW_SECS: i64 = 86_400;

#[derive(Debug)]
pub(super) enum FetchError {
    Auth,
    RateLimited,
    Other(String),
}

pub(super) enum RateLimitPolicy {
    Return,
    Retry,
}

#[derive(Default, Debug)]
pub(super) struct FetchStats {
    pub pages: usize,
    pub items: usize,
    pub events: usize,
}

pub(super) struct Snapshot {
    pub events: Vec<EventRecord>,
    pub stats: FetchStats,
}

#[derive(Deserialize)]
pub(super) struct Page {
    pub results: Vec<Value>,
    pub next: Option<String>,
}

pub(super) fn fetch_live_events(api_key: &str) -> Result<Snapshot, FetchError> {
    let from = crate::event_store::now_unix() - LIVE_WINDOW_SECS;
    let mut events = Vec::new();
    let stats = fetch_events_since(api_key, from, RateLimitPolicy::Return, |page| {
        events.extend_from_slice(page)
    })?;
    Ok(Snapshot { events, stats })
}

/// Stream pages to the consumer; a snapshot consumer publishes only after this
/// returns successfully, while history can persist completed pages as it goes.
pub(super) fn fetch_events_since(
    api_key: &str,
    from_unix: i64,
    rate_limit: RateLimitPolicy,
    on_page: impl FnMut(&[EventRecord]),
) -> Result<FetchStats, FetchError> {
    let client = Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .redirect(redirect::Policy::none())
        .build()
        .map_err(|error| FetchError::Other(error.to_string()))?;
    let mut token = HeaderValue::from_str(&format!("Token {}", api_key.trim()))
        .map_err(|error| FetchError::Other(error.to_string()))?;
    token.set_sensitive(true);
    let mut headers = HeaderMap::new();
    headers.insert(AUTHORIZATION, token);
    let mut first_page = true;
    walk_pages(
        events_url(from_unix),
        |url| {
            if !first_page {
                thread::sleep(PAGE_DELAY);
            }
            first_page = false;
            loop {
                let response = client
                    .get(url.clone())
                    .headers(headers.clone())
                    .send()
                    .map_err(|error| FetchError::Other(error.to_string()))?;
                match response.status().as_u16() {
                    200 => {}
                    401 | 403 => return Err(FetchError::Auth),
                    429 => {
                        if matches!(rate_limit, RateLimitPolicy::Retry) {
                            thread::sleep(Duration::from_secs(60));
                            continue;
                        }
                        return Err(FetchError::RateLimited);
                    }
                    code => {
                        return Err(FetchError::Other(format!(
                            "Factal API returned unexpected status {code}"
                        )));
                    }
                }
                let body = response
                    .text()
                    .map_err(|error| FetchError::Other(error.to_string()))?;
                return serde_json::from_str(&body)
                    .map_err(|error| FetchError::Other(error.to_string()));
            }
        },
        on_page,
    )
}

fn events_url(from_unix: i64) -> Url {
    let mut url = Url::parse(BASE_URL).expect("Factal base URL is valid");
    let cutoff = crate::event_store::unix_to_datetime_str(from_unix).replace(' ', "T") + "Z";
    url.query_pairs_mut()
        .append_pair("severity__gte", "2")
        .append_pair("date__gte", &cutoff);
    url
}

fn walk_pages(
    initial_url: Url,
    mut load_page: impl FnMut(&Url) -> Result<Page, FetchError>,
    mut on_page: impl FnMut(&[EventRecord]),
) -> Result<FetchStats, FetchError> {
    let mut url = initial_url.clone();
    let mut visited = HashSet::new();
    let mut ids = HashSet::new();
    let mut stats = FetchStats::default();
    loop {
        if !visited.insert(url.clone()) {
            return Err(FetchError::Other(
                "Factal pagination repeated a page URL".into(),
            ));
        }
        let page = load_page(&url)?;
        stats.pages += 1;
        stats.items += page.results.len();
        let events: Vec<_> = page
            .results
            .iter()
            .filter_map(parse_event)
            .filter(|event| ids.insert(event.id.clone()))
            .collect();
        stats.events += events.len();
        on_page(&events);
        let Some(next) = page.next.filter(|next| !next.trim().is_empty()) else {
            return Ok(stats);
        };
        let next = url
            .join(&next)
            .map_err(|error| FetchError::Other(format!("Invalid Factal next page: {error}")))?;
        if next.origin() != initial_url.origin()
            || next.path() != initial_url.path()
            || !next.username().is_empty()
            || next.password().is_some()
        {
            return Err(FetchError::Other(
                "Factal pagination left the item endpoint".into(),
            ));
        }
        url = next;
    }
}

#[cfg(test)]
#[path = "api_tests.rs"]
mod tests;
