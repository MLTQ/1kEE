use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ScrapedCameraSourceKind {
    GenericHtml,
    Opentopia,
    Webcamera24,
    WorldcamsTv,
    SkylineWebcams,
    Webcamtaxi,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ScrapedCameraSource {
    pub name: String,
    pub provider: String,
    pub kind: ScrapedCameraSourceKind,
    pub page_url: String,
    #[serde(default)]
    pub latitude: Option<f32>,
    #[serde(default)]
    pub longitude: Option<f32>,
    #[serde(default)]
    pub label_override: Option<String>,
    #[serde(default)]
    pub stream_url_override: Option<String>,
    #[serde(default)]
    pub kind_value: Option<String>,
    #[serde(default)]
    pub enabled: bool,
}

/// See `camera_source_catalog::CATALOG_TTL` — same rationale: this is called
/// several times per UI frame, so we reuse the parsed catalog briefly instead of
/// re-reading and re-parsing the JSON every frame.
const CATALOG_TTL: Duration = Duration::from_secs(2);

struct Cached {
    root_key: Option<PathBuf>,
    loaded_at: Instant,
    sources: Vec<ScrapedCameraSource>,
}

fn cache() -> &'static Mutex<Option<Cached>> {
    static C: OnceLock<Mutex<Option<Cached>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(None))
}

pub fn load_scrape_sources(selected_root: Option<&Path>) -> Vec<ScrapedCameraSource> {
    let root_key = selected_root.map(|p| p.to_path_buf());
    {
        let guard = cache().lock().unwrap();
        if let Some(c) = guard.as_ref() {
            if c.root_key == root_key && c.loaded_at.elapsed() < CATALOG_TTL {
                return c.sources.clone();
            }
        }
    }
    let sources = load_scrape_sources_uncached(selected_root);
    let mut guard = cache().lock().unwrap();
    *guard = Some(Cached {
        root_key,
        loaded_at: Instant::now(),
        sources: sources.clone(),
    });
    sources
}

fn load_scrape_sources_uncached(selected_root: Option<&Path>) -> Vec<ScrapedCameraSource> {
    source_catalog_paths(selected_root)
        .into_iter()
        .find_map(|path| {
            fs::read_to_string(&path)
                .ok()
                .and_then(|body| serde_json::from_str::<Vec<ScrapedCameraSource>>(&body).ok())
        })
        .unwrap_or_default()
        .into_iter()
        .filter(|source| source.enabled)
        .collect()
}

fn source_catalog_paths(selected_root: Option<&Path>) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(root) = selected_root {
        candidates.push(root.join("Data/camera_sources/scrape_sources.json"));
        candidates.push(root.join("Data/camera_sources/scrape_sources.jsonc"));
    }
    candidates.push(PathBuf::from("Data/camera_sources/scrape_sources.json"));
    candidates.push(PathBuf::from("Data/camera_sources/scrape_sources.jsonc"));
    candidates
}
