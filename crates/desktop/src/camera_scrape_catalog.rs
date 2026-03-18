use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};

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

pub fn load_scrape_sources(selected_root: Option<&Path>) -> Vec<ScrapedCameraSource> {
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
