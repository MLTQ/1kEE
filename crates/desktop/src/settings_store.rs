use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const SETTINGS_FILE: &str = ".1kee_settings.json";

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AppSettings {
    #[serde(default)]
    pub factal_api_key: String,
    #[serde(default)]
    pub windy_webcams_api_key: String,
    #[serde(default)]
    pub ny511_api_key: String,
    #[serde(default)]
    pub asset_root: Option<String>,
    #[serde(default)]
    pub data_root: Option<String>,
    #[serde(default)]
    pub derived_root: Option<String>,
    #[serde(default)]
    pub srtm_root: Option<String>,
    #[serde(default)]
    pub planet_path: Option<String>,
    #[serde(default)]
    pub gdal_bin_dir: Option<String>,
    /// Optional directory containing the `osmium` binary.  When absent the
    /// app searches common Homebrew / system paths automatically.
    #[serde(default)]
    pub osmium_bin_dir: Option<String>,
    /// When true, always use the Overpass API for road/feature imports even
    /// if osmium + a local planet file are both available.
    #[serde(default)]
    pub prefer_overpass: bool,
}

pub fn load_app_settings() -> AppSettings {
    settings_cache()
        .lock()
        .ok()
        .and_then(|guard| guard.clone())
        .unwrap_or_else(load_uncached)
}

pub fn save_app_settings(settings: &AppSettings) -> std::io::Result<()> {
    let path = settings_path()
        .ok_or_else(|| std::io::Error::other("unable to resolve app settings path"))?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let normalized = normalize_settings(settings.clone());
    let body = serde_json::to_string_pretty(&normalized)
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    fs::write(&path, format!("{body}\n"))?;

    if let Ok(mut guard) = settings_cache().lock() {
        *guard = Some(normalized);
    }
    Ok(())
}

pub fn load_factal_api_key() -> Option<String> {
    let trimmed = load_app_settings().factal_api_key.trim().to_owned();
    (!trimmed.is_empty()).then_some(trimmed)
}

pub fn save_factal_api_key(api_key: &str) -> std::io::Result<()> {
    let mut settings = load_app_settings();
    settings.factal_api_key = api_key.trim().to_owned();
    save_app_settings(&settings)
}

pub fn executable_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf))
}

pub fn effective_asset_root() -> Option<PathBuf> {
    load_app_settings()
        .asset_root
        .as_deref()
        .and_then(path_from_optional)
        .or_else(executable_dir)
}

pub fn configured_data_root() -> Option<PathBuf> {
    load_app_settings()
        .data_root
        .as_deref()
        .and_then(path_from_optional)
}

pub fn configured_derived_root() -> Option<PathBuf> {
    load_app_settings()
        .derived_root
        .as_deref()
        .and_then(path_from_optional)
}

pub fn configured_srtm_root() -> Option<PathBuf> {
    load_app_settings()
        .srtm_root
        .as_deref()
        .and_then(path_from_optional)
}

pub fn configured_planet_path() -> Option<PathBuf> {
    load_app_settings()
        .planet_path
        .as_deref()
        .and_then(path_from_optional)
}

pub fn configured_gdal_bin_dir() -> Option<PathBuf> {
    load_app_settings()
        .gdal_bin_dir
        .as_deref()
        .and_then(path_from_optional)
}

pub fn resolve_gdal_tool(tool: &str) -> PathBuf {
    if let Some(bin_dir) = configured_gdal_bin_dir() {
        let candidate = bin_dir.join(tool);
        if candidate.exists() {
            return candidate;
        }
    }

    PathBuf::from(tool)
}

pub fn configured_osmium_bin_dir() -> Option<PathBuf> {
    load_app_settings()
        .osmium_bin_dir
        .as_deref()
        .and_then(path_from_optional)
}

pub fn prefer_overpass() -> bool {
    load_app_settings().prefer_overpass
}

/// Resolve the `osmium` binary path.  Search order:
/// 1. Configured osmium_bin_dir in settings
/// 2. Common Homebrew prefix paths (Apple Silicon + Intel)
/// 3. Plain "osmium" on $PATH
pub fn resolve_osmium() -> PathBuf {
    if let Some(bin_dir) = configured_osmium_bin_dir() {
        let candidate = bin_dir.join("osmium");
        if candidate.exists() {
            return candidate;
        }
    }

    // Homebrew on Apple Silicon and Intel Mac
    for prefix in &["/opt/homebrew/bin/osmium", "/usr/local/bin/osmium"] {
        let p = PathBuf::from(prefix);
        if p.exists() {
            return p;
        }
    }

    PathBuf::from("osmium")
}

pub fn ensure_default_asset_layout() -> std::io::Result<()> {
    let Some(asset_root) = effective_asset_root() else {
        return Ok(());
    };

    fs::create_dir_all(&asset_root)?;
    fs::create_dir_all(asset_root.join("Data"))?;
    fs::create_dir_all(asset_root.join("Derived"))?;
    Ok(())
}

fn settings_path() -> Option<PathBuf> {
    Some(executable_dir()?.join(SETTINGS_FILE))
}

fn settings_cache() -> &'static Mutex<Option<AppSettings>> {
    static CACHE: OnceLock<Mutex<Option<AppSettings>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

fn load_uncached() -> AppSettings {
    let settings = settings_path()
        .and_then(|path| fs::read_to_string(path).ok())
        .and_then(|body| serde_json::from_str::<AppSettings>(&body).ok())
        .map(normalize_settings)
        .unwrap_or_default();

    if let Ok(mut guard) = settings_cache().lock() {
        *guard = Some(settings.clone());
    }
    settings
}

fn normalize_settings(mut settings: AppSettings) -> AppSettings {
    settings.factal_api_key = settings.factal_api_key.trim().to_owned();
    settings.windy_webcams_api_key = settings.windy_webcams_api_key.trim().to_owned();
    settings.ny511_api_key = settings.ny511_api_key.trim().to_owned();
    settings.asset_root = normalize_asset_root_owned(settings.asset_root);
    settings.data_root = normalize_named_root_owned(settings.data_root, &["Data", "data"]);
    settings.derived_root =
        normalize_named_root_owned(settings.derived_root, &["Derived", "derived"]);
    settings.srtm_root = normalize_srtm_root_owned(settings.srtm_root);
    settings.planet_path = normalize_optional_owned(settings.planet_path);
    settings.gdal_bin_dir = normalize_optional_owned(settings.gdal_bin_dir);
    settings
}

fn normalize_optional_owned(value: Option<String>) -> Option<String> {
    value.and_then(|text| {
        let trimmed = text.trim().to_owned();
        (!trimmed.is_empty()).then_some(trimmed)
    })
}

fn normalize_asset_root_owned(value: Option<String>) -> Option<String> {
    let path = normalize_optional_owned(value)?;
    let path_buf = PathBuf::from(&path);
    let Some(name) = path_buf.file_name().and_then(|name| name.to_str()) else {
        return Some(path);
    };

    if matches!(name, "Data" | "data" | "Derived" | "derived") {
        if let Some(parent) = path_buf.parent() {
            return Some(parent.display().to_string());
        }
    }

    Some(path)
}

fn normalize_named_root_owned(value: Option<String>, names: &[&str]) -> Option<String> {
    let path = normalize_optional_owned(value)?;
    let path_buf = PathBuf::from(&path);
    if !path_buf.exists() {
        return Some(path);
    }

    if path_buf
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| names.iter().any(|candidate| candidate == &name))
    {
        return Some(path);
    }

    if let Some(candidate) = names
        .iter()
        .map(|name| path_buf.join(name))
        .find(|candidate| candidate.exists())
    {
        return Some(candidate.display().to_string());
    }

    Some(path)
}

fn normalize_srtm_root_owned(value: Option<String>) -> Option<String> {
    let path = normalize_optional_owned(value)?;
    let path_buf = PathBuf::from(&path);
    if !path_buf.exists() {
        return Some(path);
    }

    if path_buf
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == "SRTM_GL1_srtm")
    {
        return Some(path);
    }

    for candidate in [
        path_buf.join("SRTM_GL1_srtm"),
        path_buf.join("srtm_gl1").join("SRTM_GL1_srtm"),
        path_buf.join("Data").join("srtm_gl1").join("SRTM_GL1_srtm"),
        path_buf.join("data").join("srtm_gl1").join("SRTM_GL1_srtm"),
    ] {
        if candidate.exists() {
            return Some(candidate.display().to_string());
        }
    }

    Some(path)
}

fn path_from_optional(text: &str) -> Option<PathBuf> {
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| PathBuf::from(trimmed))
}
