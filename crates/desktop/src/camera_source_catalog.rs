use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};

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

pub fn load_public_sources(selected_root: Option<&Path>) -> Vec<PublicCameraSource> {
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
