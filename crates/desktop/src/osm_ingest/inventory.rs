use crate::settings_store;
use osmpbf::{BlobDecode, BlobReader};
use std::fs;
use std::path::{Path, PathBuf};

use super::db::{read_runtime_counts, runtime_db_path};
use super::util::{human_bytes, yes_no};
use super::{OsmInventory, PLANET_PBF_NAME};

impl OsmInventory {
    pub fn detect_from(selected_root: Option<&Path>) -> Self {
        let planet_path = find_planet_pbf(selected_root);
        let planet_size_bytes = planet_path
            .as_ref()
            .and_then(|path| fs::metadata(path).ok())
            .map(|metadata| metadata.len())
            .unwrap_or_default();
        let runtime_db_path = runtime_db_path(selected_root);

        let (runtime_db_ready, queued_jobs, road_tiles, building_tiles, water_tiles) =
            runtime_db_path
                .as_ref()
                .filter(|path| path.exists())
                .and_then(|path| read_runtime_counts(path).ok())
                .unwrap_or((false, 0, 0, 0, 0));

        let primary_runtime_source = if road_tiles > 0 || building_tiles > 0 || water_tiles > 0 {
            "Planet OSM -> shared SQLite tile store"
        } else if runtime_db_ready {
            "Planet OSM detected, runtime schema ready"
        } else if planet_path.is_some() {
            "Planet OSM source detected"
        } else {
            "No OSM planet source detected"
        };

        Self {
            planet_path,
            planet_size_bytes,
            runtime_db_path,
            runtime_db_ready,
            queued_jobs,
            road_tiles,
            building_tiles,
            water_tiles,
            primary_runtime_source,
        }
    }

    pub fn status_label(&self) -> &'static str {
        if self.queued_jobs > 0 {
            "building"
        } else if self.runtime_db_ready {
            "ready"
        } else if self.planet_path.is_some() {
            "source"
        } else {
            "missing"
        }
    }

    pub fn status_summary(&self) -> String {
        format!(
            "Planet {} | Runtime DB {} | queued jobs {} | road tiles {} | building tiles {}",
            self.planet_path
                .as_ref()
                .map(|_| human_bytes(self.planet_size_bytes))
                .unwrap_or_else(|| "missing".into()),
            yes_no(self.runtime_db_ready),
            self.queued_jobs,
            self.road_tiles,
            self.building_tiles
        )
    }

    pub fn status_lines(&self) -> Vec<String> {
        let mut lines = vec![format!("OSM assets detected: {}", self.status_summary())];
        lines.push(format!(
            "Preferred OSM runtime source: {}",
            self.primary_runtime_source
        ));

        if let Some(planet_path) = &self.planet_path {
            lines.push(format!(
                "Planet source: {} ({})",
                planet_path.display(),
                human_bytes(self.planet_size_bytes)
            ));
        }

        if let Some(runtime_db_path) = &self.runtime_db_path {
            lines.push(format!("OSM runtime DB: {}", runtime_db_path.display()));
        }

        lines
    }
}

pub fn find_planet_pbf(selected_root: Option<&Path>) -> Option<PathBuf> {
    if let Some(configured) = settings_store::configured_planet_path() {
        if configured.exists() {
            return Some(configured);
        }
    }

    if let Some(root) = selected_root {
        if let Some(path) = find_planet_from(root) {
            return Some(path);
        }
    }

    if let Some(asset_root) = settings_store::effective_asset_root() {
        if let Some(path) = find_planet_from(&asset_root) {
            return Some(path);
        }
    }

    None
}

pub(super) fn find_planet_from(root: &Path) -> Option<PathBuf> {
    if root.is_file()
        && root
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name == PLANET_PBF_NAME)
    {
        return Some(root.to_path_buf());
    }

    if let Some(candidate) = [
        root.join(PLANET_PBF_NAME),
        root.join("Data").join(PLANET_PBF_NAME),
    ]
    .into_iter()
    .find(|candidate| candidate.exists())
    {
        return Some(candidate);
    }

    root.ancestors().find_map(|ancestor| {
        [
            ancestor.join(PLANET_PBF_NAME),
            ancestor.join("Data").join(PLANET_PBF_NAME),
            ancestor.join("data").join(PLANET_PBF_NAME),
        ]
        .into_iter()
        .find(|candidate| candidate.exists())
    })
}

#[allow(dead_code)]
pub fn validate_reader(selected_root: Option<&Path>) -> Result<(), String> {
    let path = find_planet_pbf(selected_root)
        .ok_or_else(|| "No planet-latest.osm.pbf source found for validation.".to_owned())?;
    let _reader = osmpbf::indexed::IndexedReader::from_path(&path).map_err(|error| {
        format!(
            "Failed to initialize Rust OSM PBF reader for {}: {error}",
            path.display()
        )
    })?;
    Ok(())
}

pub fn supports_locations_on_ways(selected_root: Option<&Path>) -> Result<bool, String> {
    let path = find_planet_pbf(selected_root)
        .ok_or_else(|| "No planet-latest.osm.pbf source found for validation.".to_owned())?;
    supports_locations_on_ways_for_path(&path)
}

pub(super) fn supports_locations_on_ways_for_path(path: &Path) -> Result<bool, String> {
    let mut reader = BlobReader::from_path(path).map_err(|error| error.to_string())?;
    let Some(blob) = reader.next() else {
        return Err(format!("OSM planet file {} is empty.", path.display()));
    };
    let blob = blob.map_err(|error| error.to_string())?;
    let header = match blob.decode().map_err(|error| error.to_string())? {
        BlobDecode::OsmHeader(header) => header,
        _ => {
            return Err(format!(
                "OSM planet file {} did not begin with a valid header block.",
                path.display()
            ));
        }
    };

    Ok(header
        .optional_features()
        .iter()
        .any(|feature| feature == "LocationsOnWays"))
}
