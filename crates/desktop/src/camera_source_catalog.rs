use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PublicCameraSourceKind {
    JsonArray,
    GeoJson,
    ArcGisFeatureService,
}

#[derive(Clone, Debug, Deserialize)]
pub struct PublicCameraSource {
    pub name: String,
    pub provider: String,
    pub kind: PublicCameraSourceKind,
    pub endpoint: String,
    #[serde(default)]
    pub array_field: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    pub feature_field: Option<String>,
    #[serde(default)]
    pub id_field: Option<String>,
    #[serde(default)]
    pub label_field: Option<String>,
    #[serde(default)]
    pub stream_url_field: Option<String>,
    #[serde(default)]
    pub kind_value: Option<String>,
    #[serde(default)]
    pub latitude_field: Option<String>,
    #[serde(default)]
    pub longitude_field: Option<String>,
    #[serde(default)]
    pub geometry_x_field: Option<String>,
    #[serde(default)]
    pub geometry_y_field: Option<String>,
    #[serde(default)]
    pub enabled: bool,
}

/// How long a parsed catalog is reused before re-reading from disk. The catalog
/// is a config file edited by hand, so a couple seconds of staleness is fine and
/// it spares us a disk read + JSON parse on every UI frame (this is called
/// several times per frame from `camera_registry::tick`).
const CATALOG_TTL: Duration = Duration::from_secs(2);

struct Cached {
    root_key: Option<PathBuf>,
    loaded_at: Instant,
    sources: Vec<PublicCameraSource>,
}

fn cache() -> &'static Mutex<Option<Cached>> {
    static C: OnceLock<Mutex<Option<Cached>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(None))
}

pub fn load_public_sources(selected_root: Option<&Path>) -> Vec<PublicCameraSource> {
    let root_key = selected_root.map(|p| p.to_path_buf());
    {
        let guard = cache().lock().unwrap();
        if let Some(c) = guard.as_ref() {
            if c.root_key == root_key && c.loaded_at.elapsed() < CATALOG_TTL {
                return c.sources.clone();
            }
        }
    }
    let sources = load_public_sources_uncached(selected_root);
    let mut guard = cache().lock().unwrap();
    *guard = Some(Cached {
        root_key,
        loaded_at: Instant::now(),
        sources: sources.clone(),
    });
    sources
}

fn load_public_sources_uncached(selected_root: Option<&Path>) -> Vec<PublicCameraSource> {
    source_catalog_paths(selected_root)
        .into_iter()
        .find_map(|path| {
            fs::read_to_string(&path)
                .ok()
                .and_then(|body| serde_json::from_str::<Vec<PublicCameraSource>>(&body).ok())
        })
        .unwrap_or_default()
        .into_iter()
        .filter(|source| source.enabled)
        .collect()
}

fn source_catalog_paths(selected_root: Option<&Path>) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(root) = selected_root {
        candidates.push(root.join("Data/camera_sources/public_sources.json"));
        candidates.push(root.join("Data/camera_sources/public_sources.jsonc"));
    }
    candidates.push(PathBuf::from("Data/camera_sources/public_sources.json"));
    candidates.push(PathBuf::from("Data/camera_sources/public_sources.jsonc"));
    candidates
}
