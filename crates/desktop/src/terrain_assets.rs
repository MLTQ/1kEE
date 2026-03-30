use crate::settings_store;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

pub struct TerrainInventory {
    pub gebco_topography_tiles: usize,
    pub gebco_tid_tiles: usize,
    pub natural_earth_relief: bool,
    pub srtm_tiles: usize,
    pub runtime_height_preview: bool,
    pub runtime_contours_200m: bool,
    pub runtime_contours_500m: bool,
    pub sldem2015_preview: bool,
    pub primary_runtime_source: &'static str,
}

impl TerrainInventory {
    pub fn detect_from(selected_root: Option<&Path>) -> Self {
        let data_root = find_data_root(selected_root).unwrap_or_else(|| {
            settings_store::effective_asset_root()
                .unwrap_or_default()
                .join("Data")
        });
        let derived_root = find_derived_root(selected_root).unwrap_or_else(|| {
            settings_store::effective_asset_root()
                .unwrap_or_default()
                .join("Derived")
        });
        let srtm_root = find_srtm_root(selected_root);
        let gebco_topography_tiles = count_tifs(
            data_root.join("GEBCO/gebco_2025_sub_ice_topo_geotiff"),
            "gebco_2025_sub_ice_",
        );
        let gebco_tid_tiles = count_tifs(
            data_root.join("GEBCO/gebco_2025_tid_geotiff"),
            "gebco_2025_tid_",
        );
        let natural_earth_relief = data_root
            .join("natural_earth/GRAY_HR_SR_OB_DR/GRAY_HR_SR_OB_DR.tif")
            .exists();
        let srtm_tiles = srtm_root
            .as_ref()
            .map(|root| count_tifs(root.clone(), ""))
            .unwrap_or_default();
        let runtime_height_preview = derived_root
            .join("terrain/gebco_2025_preview_4096.png")
            .exists();
        let runtime_contours_200m = derived_root
            .join("terrain/gebco_2025_contours_200m.gpkg")
            .exists();
        let runtime_contours_500m = derived_root
            .join("terrain/gebco_2025_contours_500m.gpkg")
            .exists();
        let sldem2015_preview = derived_root
            .join("terrain/sldem2015_preview_4096.png")
            .exists();

        let primary_runtime_source = if srtm_tiles > 0 {
            "SRTM streamed land tiles + GEBCO global fallback"
        } else if runtime_height_preview {
            "GEBCO runtime preview asset"
        } else if runtime_contours_200m || runtime_contours_500m {
            "GEBCO runtime contours"
        } else if gebco_topography_tiles > 0 {
            "GEBCO global terrain"
        } else if natural_earth_relief {
            "Natural Earth raster relief"
        } else {
            "No terrain assets detected"
        };

        Self {
            gebco_topography_tiles,
            gebco_tid_tiles,
            natural_earth_relief,
            srtm_tiles,
            runtime_height_preview,
            runtime_contours_200m,
            runtime_contours_500m,
            sldem2015_preview,
            primary_runtime_source,
        }
    }

    pub fn status_label(&self) -> &'static str {
        if self.runtime_height_preview || self.srtm_tiles > 0 {
            "ready"
        } else if self.gebco_topography_tiles > 0
            || self.natural_earth_relief
            || self.runtime_contours_200m
            || self.runtime_contours_500m
        {
            "partial"
        } else {
            "missing"
        }
    }

    pub fn status_summary(&self) -> String {
        format!(
            "GEBCO topo {} tiles | TID {} tiles | Natural Earth relief {} | SRTM {} tiles | Runtime height {} | 200m contours {} | 500m contours {} | SLDEM2015 preview {}",
            self.gebco_topography_tiles,
            self.gebco_tid_tiles,
            yes_no(self.natural_earth_relief),
            self.srtm_tiles,
            yes_no(self.runtime_height_preview),
            yes_no(self.runtime_contours_200m),
            yes_no(self.runtime_contours_500m),
            yes_no(self.sldem2015_preview),
        )
    }

    pub fn status_lines(&self) -> Vec<String> {
        let mut lines = vec![format!(
            "Terrain assets detected: {}",
            self.status_summary()
        )];
        lines.push(format!(
            "Preferred runtime source: {}",
            self.primary_runtime_source
        ));

        if self.srtm_tiles > 0 {
            lines.push(format!(
                "SRTM mirror detected ({} tiles) and should be streamed lazily from disk rather than preloaded.",
                self.srtm_tiles
            ));
        }

        lines
    }
}

pub fn find_data_root(selected_root: Option<&Path>) -> Option<PathBuf> {
    if let Some(configured) = settings_store::configured_data_root() {
        if let Some(normalized) = normalize_named_root(&configured, &["Data", "data"]) {
            return Some(normalized);
        }
    }
    find_named_root(selected_root, &["Data", "data"])
}

pub fn find_derived_root(selected_root: Option<&Path>) -> Option<PathBuf> {
    if let Some(configured) = settings_store::configured_derived_root() {
        if let Some(normalized) = normalize_named_root(&configured, &["Derived", "derived"]) {
            return Some(normalized);
        }
    }
    find_named_root(selected_root, &["Derived", "derived"])
}

pub fn find_srtm_root(selected_root: Option<&Path>) -> Option<PathBuf> {
    // Cache the last (selected_root, resolved) pair — this function is called per-frame
    // from is_active() and was doing full filesystem traversal every call.
    static CACHE: OnceLock<Mutex<(Option<PathBuf>, Option<PathBuf>)>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new((None, None)));
    let key = selected_root.map(Path::to_path_buf);

    if let Ok(guard) = cache.lock() {
        if guard.0 == key {
            return guard.1.clone();
        }
    }

    let result = find_srtm_root_uncached(selected_root);

    if let Ok(mut guard) = cache.lock() {
        *guard = (key, result.clone());
    }

    result
}

fn find_srtm_root_uncached(selected_root: Option<&Path>) -> Option<PathBuf> {
    if let Some(configured) = settings_store::configured_srtm_root() {
        if let Some(normalized) = find_srtm_root_from(&configured) {
            return Some(normalized);
        }
    }

    if let Some(root) = selected_root {
        if let Some(candidate) = find_srtm_root_from(root) {
            return Some(candidate);
        }
    }

    if let Some(asset_root) = settings_store::effective_asset_root() {
        if let Some(candidate) = find_srtm_root_from(&asset_root) {
            return Some(candidate);
        }
    }

    None
}

fn find_named_root(selected_root: Option<&Path>, names: &[&str]) -> Option<PathBuf> {
    if let Some(root) = selected_root {
        if let Some(candidate) = normalize_named_root(root, names) {
            return Some(candidate);
        }

        if let Some(candidate) = root
            .ancestors()
            .find_map(|ancestor| normalize_named_root(ancestor, names))
        {
            return Some(candidate);
        }
    }

    let asset_root = settings_store::effective_asset_root()?;
    normalize_named_root(&asset_root, names)
}

fn find_srtm_root_from(root: &Path) -> Option<PathBuf> {
    if let Some(candidate) = normalize_srtm_root(root) {
        return Some(candidate);
    }

    root.ancestors().find_map(|ancestor| {
        [
            ancestor.join("srtm_gl1"),
            ancestor.join("SRTM_GL1_srtm"),
            ancestor.join("data").join("srtm_gl1"),
            ancestor.join("Data").join("srtm_gl1"),
        ]
        .into_iter()
        .find_map(|candidate| normalize_srtm_root(candidate.as_path()))
    })
}

fn normalize_srtm_root(path: &Path) -> Option<PathBuf> {
    if !path.exists() {
        return None;
    }

    if path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == "SRTM_GL1_srtm")
    {
        return Some(path.to_path_buf());
    }

    let nested = path.join("SRTM_GL1_srtm");
    if nested.exists() {
        return Some(nested);
    }

    [
        path.join("srtm_gl1").join("SRTM_GL1_srtm"),
        path.join("Data").join("srtm_gl1").join("SRTM_GL1_srtm"),
        path.join("data").join("srtm_gl1").join("SRTM_GL1_srtm"),
    ]
    .into_iter()
    .find(|candidate| candidate.exists())
}

fn normalize_named_root(path: &Path, names: &[&str]) -> Option<PathBuf> {
    if !path.exists() {
        return None;
    }

    if path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| names.iter().any(|candidate| candidate == &name))
    {
        return Some(path.to_path_buf());
    }

    names
        .iter()
        .map(|name| path.join(name))
        .find(|candidate| candidate.exists())
}

fn count_tifs(root: PathBuf, prefix: &str) -> usize {
    fs::read_dir(root)
        .ok()
        .into_iter()
        .flat_map(|entries| entries.filter_map(Result::ok))
        .filter(|entry| is_matching_tif(entry.path(), prefix))
        .count()
}

fn is_matching_tif(path: PathBuf, prefix: &str) -> bool {
    let extension_ok = path.extension().and_then(|ext| ext.to_str()) == Some("tif");
    let prefix_ok = if prefix.is_empty() {
        true
    } else {
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(prefix))
    };

    extension_ok && prefix_ok
}

/// Find the SLDEM2015 JP2 file.  Checks the configured Data root, then the
/// selected root, then several well-known external-volume paths.
pub fn find_sldem_jp2(selected_root: Option<&Path>) -> Option<PathBuf> {
    let filename = "SLDEM2015 Lunar Topography.JP2";

    // Check configured / selected Data root
    if let Some(data_root) = find_data_root(selected_root) {
        let candidate = data_root.join(filename);
        if candidate.exists() {
            return Some(candidate);
        }
    }

    // Well-known external volume locations
    for prefix in &["/Volumes/Hilbert/Data", "/Volumes/Data", "/Volumes/Hilbert"] {
        let candidate = PathBuf::from(prefix).join(filename);
        if candidate.exists() {
            return Some(candidate);
        }
    }

    None
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}
