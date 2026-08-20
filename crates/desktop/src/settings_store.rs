use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const SETTINGS_FILE: &str = ".1kee_settings.json";

/// The default keeps the contour renderer byte-for-byte equivalent to the
/// visual weight it used before the operator control was added.
pub(crate) const DEFAULT_CONTOUR_STROKE_SCALE: f32 = 1.0;
pub(crate) const MIN_CONTOUR_STROKE_SCALE: f32 = 0.25;
pub(crate) const MAX_CONTOUR_STROKE_SCALE: f32 = 3.0;
/// The historical globe contour width at a `1×` multiplier, expressed in
/// egui logical points. New pixel-width selections convert through this value
/// so the local renderer keeps its established major/minor hierarchy.
pub(crate) const LEGACY_CONTOUR_STROKE_WIDTH_POINTS: f32 = 1.15;
/// A full GPU contour stroke must retain at least one physical pixel of core
/// width so the anti-aliased line does not disappear at the lower endpoint.
pub(crate) const MIN_CONTOUR_STROKE_WIDTH_PX: f32 = 1.0;
/// The new pixel-width control supports a comfortably wider range than the
/// former multiplier without changing any legacy saved appearance.
pub(crate) const MAX_CONTOUR_STROKE_WIDTH_PX: f32 = 16.0;
pub(crate) const DEFAULT_EYES_ON_MAX_PAGES: u8 = 1;
pub(crate) const MAX_EYES_ON_MAX_PAGES: u8 = 5;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AppSettings {
    #[serde(default)]
    pub factal_api_key: String,
    #[serde(default)]
    pub windy_webcams_api_key: String,
    #[serde(default)]
    pub ny511_api_key: String,
    /// Explicit opt-in for the Project Eyes On-compatible Insecam directory
    /// pipeline. Disabled by default because it performs live public-network
    /// requests to directory-listed feeds.
    #[serde(default)]
    pub eyes_on_enabled: bool,
    /// Optional ISO alpha-2 country code used to scope directory discovery.
    #[serde(default)]
    pub eyes_on_country_code: String,
    /// Maximum directory pages fetched per poll. Normalized to a small bound.
    #[serde(default = "default_eyes_on_max_pages")]
    pub eyes_on_max_pages: u8,
    #[serde(default)]
    pub aisstream_api_key: String,
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
    /// Historical multiplier for contour-derived stroke widths. Retained so
    /// saved settings from before the pixel-width control keep their exact
    /// legacy visual weight until the operator chooses a pixel value.
    #[serde(default = "default_contour_stroke_scale")]
    pub contour_stroke_scale: f32,
    /// Physical-pixel width selected by the contour control. `None` means use
    /// `contour_stroke_scale` as a backwards-compatible legacy fallback.
    #[serde(default)]
    pub contour_stroke_width_px: Option<f32>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            factal_api_key: String::new(),
            windy_webcams_api_key: String::new(),
            ny511_api_key: String::new(),
            eyes_on_enabled: false,
            eyes_on_country_code: String::new(),
            eyes_on_max_pages: DEFAULT_EYES_ON_MAX_PAGES,
            aisstream_api_key: String::new(),
            asset_root: None,
            data_root: None,
            derived_root: None,
            srtm_root: None,
            planet_path: None,
            gdal_bin_dir: None,
            osmium_bin_dir: None,
            prefer_overpass: false,
            contour_stroke_scale: DEFAULT_CONTOUR_STROKE_SCALE,
            contour_stroke_width_px: None,
        }
    }
}

fn default_contour_stroke_scale() -> f32 {
    DEFAULT_CONTOUR_STROKE_SCALE
}

fn default_eyes_on_max_pages() -> u8 {
    DEFAULT_EYES_ON_MAX_PAGES
}

pub(crate) fn normalize_eyes_on_country_code(value: &str) -> String {
    let normalized = value.trim().to_ascii_uppercase();
    if normalized.len() == 2 && normalized.bytes().all(|byte| byte.is_ascii_alphabetic()) {
        normalized
    } else {
        String::new()
    }
}

pub(crate) fn normalize_eyes_on_max_pages(value: u8) -> u8 {
    value.clamp(1, MAX_EYES_ON_MAX_PAGES)
}

/// Clamp persisted/operator input to the supported visual range. Invalid
/// values fall back to the legacy 1× appearance instead of producing an
/// invisible or malformed GPU stroke.
pub(crate) fn normalize_contour_stroke_scale(value: f32) -> f32 {
    if !value.is_finite() || value <= 0.0 {
        DEFAULT_CONTOUR_STROKE_SCALE
    } else {
        value.clamp(MIN_CONTOUR_STROKE_SCALE, MAX_CONTOUR_STROKE_SCALE)
    }
}

/// Normalize a new operator-selected physical contour width. Invalid direct
/// setter input still resolves to the visible one-pixel minimum; malformed
/// persisted optional values instead fall back to the legacy field below.
pub(crate) fn normalize_contour_stroke_width_px(value: f32) -> f32 {
    if !value.is_finite() {
        MIN_CONTOUR_STROKE_WIDTH_PX
    } else {
        value.clamp(MIN_CONTOUR_STROKE_WIDTH_PX, MAX_CONTOUR_STROKE_WIDTH_PX)
    }
}

fn normalize_optional_contour_stroke_width_px(value: Option<f32>) -> Option<f32> {
    value
        .filter(|width_px| width_px.is_finite() && *width_px > 0.0)
        .map(normalize_contour_stroke_width_px)
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

#[allow(dead_code)]
pub fn load_factal_api_key() -> Option<String> {
    let trimmed = load_app_settings().factal_api_key.trim().to_owned();
    (!trimmed.is_empty()).then_some(trimmed)
}

#[allow(dead_code)]
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

/// Path for the event history SQLite database — sibling of the settings file.
pub fn event_db_path() -> Option<PathBuf> {
    Some(executable_dir()?.join(".1kee_events.sqlite"))
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
        .unwrap_or_else(|| normalize_settings(AppSettings::default()));

    if let Ok(mut guard) = settings_cache().lock() {
        *guard = Some(settings.clone());
    }
    settings
}

fn normalize_settings(mut settings: AppSettings) -> AppSettings {
    settings.factal_api_key = settings.factal_api_key.trim().to_owned();
    settings.windy_webcams_api_key = settings.windy_webcams_api_key.trim().to_owned();
    settings.ny511_api_key = settings.ny511_api_key.trim().to_owned();
    settings.eyes_on_country_code = normalize_eyes_on_country_code(&settings.eyes_on_country_code);
    settings.eyes_on_max_pages = normalize_eyes_on_max_pages(settings.eyes_on_max_pages);
    settings.aisstream_api_key = settings.aisstream_api_key.trim().to_owned();
    settings.asset_root = normalize_asset_root_owned(settings.asset_root);
    settings.data_root = normalize_named_root_owned(settings.data_root, &["Data", "data"]);
    settings.derived_root =
        normalize_named_root_owned(settings.derived_root, &["Derived", "derived"]);
    settings.srtm_root = normalize_srtm_root_owned(settings.srtm_root);
    settings.planet_path = normalize_optional_owned(settings.planet_path);
    settings.gdal_bin_dir = normalize_optional_owned(settings.gdal_bin_dir);
    settings.contour_stroke_scale = normalize_contour_stroke_scale(settings.contour_stroke_scale);
    settings.contour_stroke_width_px =
        normalize_optional_contour_stroke_width_px(settings.contour_stroke_width_px);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contour_stroke_settings_default_to_the_legacy_visual_weight() {
        let settings: AppSettings = serde_json::from_str("{}").expect("valid legacy settings");
        assert_eq!(settings.contour_stroke_scale, DEFAULT_CONTOUR_STROKE_SCALE);
        assert_eq!(settings.contour_stroke_width_px, None);
        assert!(!settings.eyes_on_enabled);
        assert_eq!(settings.eyes_on_country_code, "");
        assert_eq!(settings.eyes_on_max_pages, DEFAULT_EYES_ON_MAX_PAGES);
    }

    #[test]
    fn eyes_on_scope_is_normalized_to_safe_small_values() {
        assert_eq!(normalize_eyes_on_country_code(" us "), "US");
        assert_eq!(normalize_eyes_on_country_code("USA"), "");
        assert_eq!(normalize_eyes_on_country_code("1!"), "");
        assert_eq!(normalize_eyes_on_max_pages(0), 1);
        assert_eq!(normalize_eyes_on_max_pages(99), MAX_EYES_ON_MAX_PAGES);
    }

    #[test]
    fn multiplier_only_settings_remain_on_the_legacy_migration_path() {
        let settings: AppSettings = serde_json::from_str(r#"{"contour_stroke_scale": 0.25}"#)
            .expect("valid legacy settings");
        let normalized = normalize_settings(settings);

        assert_eq!(normalized.contour_stroke_scale, MIN_CONTOUR_STROKE_SCALE);
        assert_eq!(normalized.contour_stroke_width_px, None);
    }

    #[test]
    fn contour_stroke_scale_rejects_invalid_and_out_of_range_values() {
        assert_eq!(
            normalize_contour_stroke_scale(f32::NAN),
            DEFAULT_CONTOUR_STROKE_SCALE
        );
        assert_eq!(
            normalize_contour_stroke_scale(0.0),
            DEFAULT_CONTOUR_STROKE_SCALE
        );
        assert_eq!(
            normalize_contour_stroke_scale(0.1),
            MIN_CONTOUR_STROKE_SCALE
        );
        assert_eq!(
            normalize_contour_stroke_scale(10.0),
            MAX_CONTOUR_STROKE_SCALE
        );
    }

    #[test]
    fn contour_stroke_width_uses_a_visible_pixel_range() {
        assert_eq!(
            normalize_contour_stroke_width_px(f32::NAN),
            MIN_CONTOUR_STROKE_WIDTH_PX
        );
        assert_eq!(
            normalize_contour_stroke_width_px(0.5),
            MIN_CONTOUR_STROKE_WIDTH_PX
        );
        assert_eq!(
            normalize_contour_stroke_width_px(99.0),
            MAX_CONTOUR_STROKE_WIDTH_PX
        );
        assert_eq!(normalize_optional_contour_stroke_width_px(Some(0.0)), None);
    }
}
