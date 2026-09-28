//! Small, immutable offshore facility inventories stored alongside tiled layers.
use crate::{Key, Reader, Writer};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

pub const FILE_NAME: &str = "platforms.1ka";
pub const MANIFEST: &str = "platforms.manifest.v1";
pub const KEY: Key = Key {
    body: 0,
    layer: *b"RIGS",
    grid: 4,
    level: 0,
    y: 0,
    x: 0,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Platform {
    pub source: String,
    pub source_id: String,
    pub name: String,
    pub operator: String,
    pub country: String,
    pub product: String,
    pub kind: String,
    pub function: String,
    pub status: String,
    pub installed: String,
    pub removed: String,
    pub water_depth_m: Option<f32>,
    pub historical: bool,
    pub planned: bool,
    pub support: bool,
    pub lat: f32,
    pub lon: f32,
}

#[derive(Serialize, Deserialize)]
pub struct Inventory {
    pub version: u32,
    pub manifest: serde_json::Value,
    pub platforms: Vec<Platform>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Filter {
    pub bsee: bool,
    pub emodnet: bool,
    pub historical: bool,
    pub planned: bool,
    pub support: bool,
}
impl Default for Filter {
    fn default() -> Self {
        Self {
            bsee: true,
            emodnet: true,
            historical: false,
            planned: false,
            support: false,
        }
    }
}
impl Filter {
    pub fn accepts(self, p: &Platform) -> bool {
        let source = match p.source.as_str() {
            "bsee" => self.bsee,
            "emodnet" => self.emodnet,
            _ => false,
        };
        source
            && (!p.historical || self.historical)
            && (!p.planned || self.planned)
            && (!p.support || self.support)
    }
}

pub fn decode(bytes: &[u8]) -> Result<Inventory, String> {
    if bytes.len() > 32 * 1024 * 1024 {
        return Err("Platform inventory exceeds 32 MiB".into());
    }
    let inventory: Inventory = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    if inventory.version != 1
        || inventory.manifest["version"] != 1
        || !inventory.manifest["sources"].is_array()
    {
        return Err("Unsupported platform inventory".into());
    }
    if inventory.platforms.is_empty() || inventory.platforms.len() > 100_000 {
        return Err("Invalid platform count".into());
    }
    let mut ids = HashSet::new();
    for p in &inventory.platforms {
        if !p.lat.is_finite()
            || !p.lon.is_finite()
            || p.lat.abs() > 90.0
            || p.lon.abs() > 180.0
            || p.water_depth_m.is_some_and(|d| !d.is_finite())
            || !matches!(p.source.as_str(), "bsee" | "emodnet")
            || p.source_id.is_empty()
            || !ids.insert((&p.source, &p.source_id))
        {
            return Err(format!(
                "Invalid or duplicate platform {}/{}",
                p.source, p.source_id
            ));
        }
    }
    Ok(inventory)
}

pub fn load(reader: &Reader) -> Result<Inventory, String> {
    let manifest = reader
        .metadata(MANIFEST)?
        .ok_or("No platform inventory in archive")?;
    let data = decode(&reader.get(KEY)?.ok_or("Missing platform payload")?)?;
    if serde_json::from_str::<serde_json::Value>(&manifest).map_err(|e| e.to_string())?
        != data.manifest
    {
        return Err("Platform manifest mismatch".into());
    }
    Ok(data)
}

pub fn copy_into(reader: &Reader, writer: &mut Writer) -> Result<usize, String> {
    reader
        .connection
        .execute_batch("BEGIN DEFERRED")
        .map_err(|e| e.to_string())?;
    let result = (|| {
        let data = load(reader)?;
        let bytes = reader.get(KEY)?.ok_or("Missing platform payload")?;
        writer.put_batch(&[(KEY, bytes)], None)?;
        writer.metadata(MANIFEST, &data.manifest.to_string())?;
        Ok(1)
    })();
    let _ = reader.connection.execute_batch("ROLLBACK");
    result
}

struct Stage(PathBuf);
impl Drop for Stage {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
        let _ = fs::remove_file(format!("{}-journal", self.0.display()));
    }
}

pub fn build(input: &Path, output: &Path) -> Result<usize, String> {
    if output.exists() {
        return Err(format!("Destination already exists: {}", output.display()));
    }
    if fs::metadata(input).map_err(|e| e.to_string())?.len() > 32 * 1024 * 1024 {
        return Err("Platform input exceeds 32 MiB".into());
    }
    let bytes = fs::read(input).map_err(|e| e.to_string())?;
    let data = decode(&bytes)?;
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let stage = Stage(parent.join(format!(".platforms-{}-{nonce}.part", std::process::id())));
    let mut writer = Writer::create(&stage.0)?;
    writer.put_batch(&[(KEY, bytes)], None)?;
    writer.metadata(MANIFEST, &data.manifest.to_string())?;
    writer.finish()?;
    fs::hard_link(&stage.0, output).map_err(|e| e.to_string())?;
    Ok(data.platforms.len())
}

#[cfg(test)]
#[path = "platforms_tests.rs"]
mod tests;
