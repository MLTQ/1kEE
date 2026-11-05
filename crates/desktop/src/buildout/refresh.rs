use super::{PROJECT_URL, catalog};
use catalog::Catalog;
use reqwest::{
    StatusCode,
    blocking::Client,
    header::{ETAG, IF_NONE_MATCH},
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{Read, Write},
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub(super) const DAY: Duration = Duration::from_secs(86400);
const PAGE_LIMIT: u64 = 2 << 20;
const ASSET_LIMIT: u64 = 8 << 20;
const MAX_ASSETS: usize = 24;

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Snapshot {
    pub catalog: Catalog,
    pub checked: u64,
    etag: Option<String>,
    assets: Vec<String>,
}

pub(super) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

impl Snapshot {
    pub fn remaining(&self) -> Duration {
        now()
            .checked_sub(self.checked)
            .map_or(Duration::ZERO, |age| {
                DAY.saturating_sub(Duration::from_secs(age))
            })
    }
}

pub(super) fn read_cache(path: &Path) -> Option<Snapshot> {
    let reader = fs::File::open(path).ok()?.take(ASSET_LIMIT);
    let snapshot: Snapshot = serde_json::from_reader(reader).ok()?;
    (snapshot.checked <= now().saturating_add(DAY.as_secs()) && snapshot.catalog.valid())
        .then_some(snapshot)
}

pub(super) fn write_cache(path: &Path, snapshot: &Snapshot) -> Result<(), String> {
    let dir = path.parent().ok_or("missing cache directory")?;
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir).map_err(|e| e.to_string())?;
    serde_json::to_writer(&mut tmp, snapshot).map_err(|e| e.to_string())?;
    tmp.flush().map_err(|e| e.to_string())?;
    tmp.persist(path).map_err(|e| e.to_string())?;
    Ok(())
}

fn bounded(response: reqwest::blocking::Response, limit: u64) -> Result<String, String> {
    let mut bytes = Vec::new();
    response
        .error_for_status()
        .map_err(|e| e.to_string())?
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > limit {
        return Err("buildout asset exceeded size limit".into());
    }
    String::from_utf8(bytes).map_err(|e| e.to_string())
}

pub(super) struct Validator<'a> {
    pub etag: Option<&'a str>,
    pub assets: &'a [String],
}

pub(super) enum Update {
    Unchanged(Option<String>),
    Changed(Snapshot),
}

pub(super) fn fetch(previous: Option<Validator<'_>>) -> Result<Update, String> {
    let client = Client::builder()
        .user_agent("1kEE AI buildout layer")
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())?;
    query(&client, PROJECT_URL, "https://www.zackslab.com", previous)
}

pub(super) fn validator(snapshot: &Snapshot) -> Validator<'_> {
    Validator {
        etag: snapshot.etag.as_deref(),
        assets: &snapshot.assets,
    }
}

pub(super) fn revalidated(snapshot: &mut Snapshot, etag: Option<String>) {
    snapshot.checked = now();
    snapshot.etag = etag;
}

pub(super) fn query(
    client: &Client,
    page: &str,
    origin: &str,
    previous: Option<Validator<'_>>,
) -> Result<Update, String> {
    let mut request = client.get(page);
    let old_etag = previous.as_ref().and_then(|v| v.etag);
    if let Some(tag) = old_etag {
        request = request.header(IF_NONE_MATCH, tag);
    }
    let response = request.send().map_err(|e| e.to_string())?;
    let etag = response
        .headers()
        .get(ETAG)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    if response.status() == StatusCode::NOT_MODIFIED && old_etag.is_some() {
        return Ok(Update::Unchanged(
            etag.or_else(|| old_etag.map(str::to_owned)),
        ));
    }
    if response.status() != StatusCode::OK {
        return Err(format!("buildout page returned {}", response.status()));
    }
    let html = bounded(response, PAGE_LIMIT)?;
    let assets = catalog::assets(&html);
    if let Some(v) = previous
        && !v.assets.is_empty()
        && v.assets.iter().all(|path| {
            path.starts_with("/_next/static/immutable/") && assets.contains(&path.as_str())
        })
    {
        return Ok(Update::Unchanged(etag));
    }
    let mut last_error = None;
    let mut catalog = Catalog::default();
    let mut contributors = Vec::new();
    for asset in assets.into_iter().take(MAX_ASSETS) {
        let result = client
            .get(format!("{origin}{asset}"))
            .send()
            .map_err(|e| e.to_string())
            .and_then(|response| bounded(response, ASSET_LIMIT));
        match result {
            Ok(script) => {
                if catalog.ingest(&script) {
                    contributors.push(asset.to_owned());
                    if catalog.valid() {
                        catalog
                            .news
                            .sort_by(|a, b| b.date.cmp(&a.date).then(a.id.cmp(&b.id)));
                        return Ok(Update::Changed(Snapshot {
                            catalog,
                            assets: contributors,
                            etag,
                            checked: now(),
                        }));
                    }
                }
            }
            Err(e) => last_error = Some(e),
        }
    }
    Err(last_error.unwrap_or_else(|| {
        "the published assets did not contain complete campus, rule and news tables".into()
    }))
}
