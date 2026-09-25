use super::*;
use reqwest::Url;
use reqwest::blocking::{Client, Response};
use serde_json::Value;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex, OnceLock};

fn client() -> &'static Client {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        Client::builder()
            .user_agent("1kEE terrain viewer")
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(90))
            .build()
            .expect("elevation HTTP client")
    })
}

fn get(url: &str) -> Result<Response> {
    validate_url(url)?;
    let response = client()
        .get(url)
        .timeout(remaining()?.min(Duration::from_secs(90)))
        .send()
        .map_err(|e| Error::Failed(e.to_string()))?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Err(Error::NoCoverage);
    }
    response
        .error_for_status()
        .map_err(|e| Error::Failed(e.to_string()))
}

/// Catalog data cannot redirect GDAL to a local file or an unrelated host.
pub(super) fn validate_url(url: &str) -> Result<()> {
    let url = Url::parse(url).map_err(|e| Error::Failed(e.to_string()))?;
    let allowed = matches!(
        url.host_str(),
        Some(
            "environment.data.gov.uk"
                | "service.pdok.nl"
                | "data.geopf.fr"
                | "data.geo.admin.ch"
                | "cyberjapandata.gsi.go.jp"
                | "nz-elevation.s3-ap-southeast-2.amazonaws.com"
                | "nz-elevation.s3.ap-southeast-2.amazonaws.com"
        )
    );
    if url.scheme() != "https" || !allowed || !url.username().is_empty() || url.password().is_some()
    {
        return Err(Error::Failed("unexpected elevation resource URL".into()));
    }
    Ok(())
}

pub(super) fn bytes(url: &str, limit: usize) -> Result<Vec<u8>> {
    let response = get(url)?;
    let mut bytes = Vec::new();
    response.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(Error::Failed("elevation response too large".into()));
    }
    Ok(bytes)
}

pub(super) fn download(
    url: &str,
    path: &Path,
    progress: &mut dyn FnMut(u64, Option<u64>),
) -> Result<()> {
    const LIMIT: u64 = 64 * 1024 * 1024;
    let mut response = get(url)?;
    let total = response.content_length();
    if total.is_some_and(|n| n > LIMIT) {
        return Err(Error::Failed("elevation raster too large".into()));
    }
    let mut file = std::fs::File::create(path)?;
    let mut buffer = [0u8; 65536];
    let mut done = 0;
    loop {
        let n = response.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        done += n as u64;
        if done > LIMIT {
            return Err(Error::Failed("elevation raster too large".into()));
        }
        file.write_all(&buffer[..n])?;
        progress(done, total);
    }
    if total.is_some_and(|n| n != done) {
        return Err(Error::Failed("incomplete elevation raster".into()));
    }
    file.flush()?;
    Ok(())
}

pub(super) fn json(url: &str) -> Result<Arc<Value>> {
    remaining()?;
    type Entry = (Instant, usize, Arc<Value>);
    static CACHE: OnceLock<Mutex<HashMap<String, Entry>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some((time, _, value)) = cache.lock().unwrap().get(url)
        && time.elapsed() < Duration::from_secs(600)
    {
        return Ok(Arc::clone(value));
    }
    let bytes = bytes(url, 4 * 1024 * 1024)?;
    let value = Arc::new(serde_json::from_slice(&bytes).map_err(|e| Error::Failed(e.to_string()))?);
    let mut cache = cache.lock().unwrap();
    if cache.values().map(|(_, size, _)| size).sum::<usize>() + bytes.len() > 16 * 1024 * 1024 {
        cache.clear();
    }
    cache.insert(
        url.to_owned(),
        (Instant::now(), bytes.len(), Arc::clone(&value)),
    );
    Ok(value)
}

pub(super) fn resolve(base: &str, href: &str) -> Result<String> {
    let url = Url::parse(base)
        .and_then(|u| u.join(href))
        .map_err(|e| Error::Failed(e.to_string()))?
        .to_string();
    validate_url(&url)?;
    Ok(url)
}
