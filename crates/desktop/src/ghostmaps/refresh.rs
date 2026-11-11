//! The tree is the daily index. A blob SHA identifies the exact KMZ to fetch,
//! even if main moves between the index request and the download.
use super::export_name;
use crate::model::{EventRecord, GeoJsonLayer};
use reqwest::{
    StatusCode,
    blocking::Client,
    header::{ACCEPT, ETAG, IF_NONE_MATCH},
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub(super) const API: &str = "https://api.github.com/repos/s2underground/GhostMaps/git";
pub(super) const DAY: Duration = Duration::from_secs(86400);
const KMZ_LIMIT: u64 = 80 << 20;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
pub(super) type WithWarning<T> = (T, Option<String>);

#[derive(Default, Serialize, Deserialize)]
pub(super) struct Catalog {
    pub checked: u64,
    etag: Option<String>,
    pub files: BTreeMap<String, String>,
}

impl Catalog {
    pub fn remaining(&self) -> Duration {
        now()
            .checked_sub(self.checked)
            .map_or(Duration::ZERO, |age| {
                DAY.saturating_sub(Duration::from_secs(age))
            })
    }
}

pub(super) struct Product {
    pub sha: String,
    pub layers: Vec<GeoJsonLayer>,
    pub reports: BTreeMap<String, Vec<EventRecord>>,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub(super) fn open(path: &Path) -> Result<Connection> {
    std::fs::create_dir_all(path.parent().ok_or("missing cache directory")?)?;
    let db = Connection::open(path)?;
    db.execute_batch("CREATE TABLE IF NOT EXISTS catalog(id INTEGER PRIMARY KEY, json TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS exports(path TEXT PRIMARY KEY, sha TEXT NOT NULL, kmz BLOB NOT NULL);")?;
    Ok(db)
}

pub(super) fn client() -> Result<Client> {
    Ok(Client::builder()
        .user_agent("1kEE GhostMaps layers")
        .timeout(Duration::from_secs(30))
        .build()?)
}

fn bounded(reader: impl Read, limit: u64) -> Result<Vec<u8>> {
    let mut data = Vec::new();
    reader.take(limit + 1).read_to_end(&mut data)?;
    if data.len() as u64 > limit {
        return Err("response exceeds size limit".into());
    }
    Ok(data)
}

pub(super) fn catalog(db: &Connection, client: &Client, api: &str) -> Result<WithWarning<Catalog>> {
    let json: Option<String> = db
        .query_row("SELECT json FROM catalog WHERE id=1", [], |r| r.get(0))
        .optional()?;
    let cached: Option<Catalog> = json.and_then(|s| serde_json::from_str(&s).ok());
    let cached = match cached {
        Some(c) if !c.remaining().is_zero() => return Ok((c, None)),
        other => other,
    };
    let tag = cached.as_ref().and_then(|c| c.etag.as_deref());
    let mut updated = match query_tree(client, api, tag) {
        Ok(Some(new)) => new,
        Ok(None) => cached.ok_or("missing catalog for 304 response")?,
        Err(e) => {
            return match cached {
                Some(old) => Ok((old, Some(format!("Using cached catalog · {e}")))),
                None => Err(e),
            };
        }
    };
    updated.checked = now();
    db.execute(
        "INSERT OR REPLACE INTO catalog VALUES (1, ?1)",
        [serde_json::to_string(&updated)?],
    )?;
    Ok((updated, None))
}

fn query_tree(client: &Client, api: &str, tag: Option<&str>) -> Result<Option<Catalog>> {
    #[derive(Deserialize)]
    struct Tree {
        truncated: bool,
        tree: Vec<Entry>,
    }
    #[derive(Deserialize)]
    struct Entry {
        path: String,
        sha: String,
        #[serde(rename = "type")]
        kind: String,
    }

    let mut request = client.get(format!("{api}/trees/main?recursive=1"));
    if let Some(tag) = tag {
        request = request.header(IF_NONE_MATCH, tag);
    }
    let response = request.send()?;
    if response.status() == StatusCode::NOT_MODIFIED && tag.is_some() {
        return Ok(None);
    }
    let response = response.error_for_status()?;
    let etag = response
        .headers()
        .get(ETAG)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let tree: Tree = serde_json::from_slice(&bounded(response, 8 << 20)?)?;
    if tree.truncated {
        return Err("GitHub returned an incomplete repository tree".into());
    }
    let mut files = BTreeMap::new();
    for entry in tree.tree {
        if entry.kind == "blob" && entry.path.to_ascii_lowercase().ends_with(".kmz") {
            if !valid_sha(&entry.sha) {
                return Err("invalid blob SHA in repository tree".into());
            }
            files.insert(entry.path, entry.sha);
        }
    }
    Ok(Some(Catalog {
        checked: 0,
        etag,
        files,
    }))
}

fn valid_sha(sha: &str) -> bool {
    sha.len() == 40 && sha.bytes().all(|c| c.is_ascii_hexdigit())
}

pub(super) fn decode(path: &str, sha: &str, bytes: &[u8]) -> Result<Product> {
    let mut hash = Sha1::new();
    hash.update(format!("blob {}\0", bytes.len()));
    hash.update(bytes);
    if format!("{:x}", hash.finalize()) != sha {
        return Err("KMZ did not match its published Git blob SHA".into());
    }
    let mut reports = BTreeMap::new();
    let layers =
        GeoJsonLayer::kmz_folder_layers(&export_name(path), bytes, |folder, node, features| {
            super::reports::collect(&mut reports, folder, node, features)
        })?;
    Ok(Product {
        sha: sha.to_owned(),
        layers,
        reports,
    })
}

pub(super) fn product(
    db: &Connection,
    client: &Client,
    api: &str,
    path: &str,
    sha: &str,
) -> Result<WithWarning<Product>> {
    let cached: Option<(String, Vec<u8>)> = db
        .query_row("SELECT sha, kmz FROM exports WHERE path=?1", [path], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .optional()?;
    if let Some((old_sha, bytes)) = &cached
        && old_sha == sha
        && let Ok(product) = decode(path, sha, bytes)
    {
        return Ok((product, None));
    }
    let download = || -> Result<Product> {
        if !valid_sha(sha) {
            return Err("invalid blob SHA".into());
        }
        let response = client
            .get(format!("{api}/blobs/{sha}"))
            .header(ACCEPT, "application/vnd.github.raw+json")
            .send()?
            .error_for_status()?;
        let bytes = bounded(response, KMZ_LIMIT)?;
        let product = decode(path, sha, &bytes)?;
        // Publish the bytes and their identity together, only after validation.
        db.execute(
            "INSERT OR REPLACE INTO exports VALUES (?1, ?2, ?3)",
            params![path, sha, bytes],
        )?;
        Ok(product)
    };
    match download() {
        Ok(product) => Ok((product, None)),
        Err(e) => {
            if let Some((old_sha, bytes)) = cached
                && let Ok(old) = decode(path, &old_sha, &bytes)
            {
                return Ok((old, Some(format!("Showing cached export · {e}"))));
            }
            Err(e)
        }
    }
}
