//! Public oil/gas routes, independent of OSM IDs and the existing PIPE layer.
use crate::{Key, Reader, Writer};
use cell_format::{CellFeature, CellPoint};
use serde::{Deserialize, Serialize};

pub mod build;
mod geometry;
#[cfg(test)]
mod tests;

pub const FILE_NAME: &str = "pipelines.1ka";
pub const TAG: [u8; 4] = *b"FUEL";
pub const MANIFEST: &str = "pipelines.manifest.v1";
pub const DIVISIONS: i32 = 4;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Info {
    pub source: String,
    pub source_id: String,
    pub name: String,
    pub operator: String,
    pub owner: String,
    pub product: String,
    pub status: String,
    pub accuracy: String,
    pub historical: bool,
    pub planned: bool,
}

pub struct Feature {
    pub info: Info,
    pub points: Vec<CellPoint>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Filter {
    pub eia: bool,
    pub gem: bool,
    pub bsee: bool,
    pub gas: bool,
    pub oil: bool,
    pub liquids: bool,
    pub planned: bool,
    pub historical: bool,
}

impl Default for Filter {
    fn default() -> Self {
        Self {
            eia: true,
            gem: true,
            bsee: true,
            gas: true,
            oil: true,
            liquids: true,
            planned: false,
            historical: false,
        }
    }
}

impl Filter {
    pub fn accepts(self, info: &Info) -> bool {
        let source = match info.source.as_str() {
            "bsee" => self.bsee,
            "gem-gas" | "gem-oil" => self.gem,
            s if s.starts_with("eia-") => self.eia,
            _ => false,
        };
        let product = match info.product.as_str() {
            "gas" => self.gas,
            "oil" => self.oil,
            _ => self.liquids,
        };
        source
            && product
            && (!info.historical || self.historical)
            && (!info.planned || self.planned)
    }
}

pub fn key(level: i32, y: i32, x: i32) -> Key {
    Key {
        body: 0,
        layer: TAG,
        grid: 3,
        level,
        y,
        x,
    }
}

pub fn decode(bytes: &[u8]) -> Result<Vec<Feature>, String> {
    let features =
        cell_format::read::read_single_chunk(bytes, TAG).ok_or("Invalid pipeline tile")?;
    features
        .into_iter()
        .map(|f| {
            let info: Info =
                serde_json::from_str(f.name.as_deref().ok_or("Missing pipeline attributes")?)
                    .map_err(|e| e.to_string())?;
            if f.points.len() < 2
                || f.points.iter().any(|p| {
                    !p.lat.is_finite()
                        || !p.lon.is_finite()
                        || p.lat.abs() > 90.0
                        || p.lon.abs() > 180.0
                })
            {
                return Err("Invalid pipeline coordinates".into());
            }
            Ok(Feature {
                info,
                points: f.points,
            })
        })
        .collect()
}

/// Bounds are [min_lat, max_lat, min_lon, max_lon]. Large views use baked overview.
pub fn load(reader: &Reader, bounds: Option<[f32; 4]>) -> Result<Vec<Feature>, String> {
    if reader.metadata(MANIFEST)?.is_none() {
        return Err("No public pipelines in this archive".into());
    }
    let Some(b) = bounds else {
        return decode(
            &reader
                .get(key(1, 0, 0))?
                .ok_or("Missing pipeline overview")?,
        );
    };
    if b.iter().any(|v| !v.is_finite()) || b[0] > b[1] || b[2] > b[3] {
        return Err("Invalid pipeline view".into());
    }
    let y0 = ((b[0] * 4.0).floor() as i32).clamp(-360, 359);
    let y1 = ((b[1] * 4.0).floor() as i32).clamp(-360, 359);
    let x0 = ((b[2] * 4.0).floor() as i32).clamp(-720, 719);
    let x1 = ((b[3] * 4.0).floor() as i32).clamp(-720, 719);
    if (y1 - y0 + 1) * (x1 - x0 + 1) > 4096 {
        return load(reader, None);
    }
    let mut features = Vec::new();
    for y in y0..=y1 {
        for x in x0..=x1 {
            if let Some(bytes) = reader.get(key(0, y, x))? {
                features.extend(decode(&bytes)?);
            }
        }
    }
    Ok(features)
}

/// Copy just this namespace into a new world snapshot, verifying every checksum.
pub fn copy_into(reader: &Reader, writer: &mut Writer) -> Result<usize, String> {
    reader
        .connection
        .execute_batch("BEGIN DEFERRED")
        .map_err(|e| e.to_string())?;
    let result = (|| {
        let manifest = reader
            .metadata(MANIFEST)?
            .ok_or("No public pipeline manifest")?;
        let mut stmt = reader.connection.prepare("SELECT level,y,x FROM tiles WHERE body=0 AND layer=?1 AND grid=3 ORDER BY level,y,x").map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([&TAG[..]], |r| {
                Ok((
                    r.get::<_, i32>(0)?,
                    r.get::<_, i32>(1)?,
                    r.get::<_, i32>(2)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        let mut count = 0;
        let mut batch = Vec::new();
        for row in rows {
            let (level, y, x) = row.map_err(|e| e.to_string())?;
            if level != 0 && level != 1 {
                return Err("Unsupported pipeline level".into());
            }
            let key = key(level, y, x);
            let bytes = reader
                .get(key)?
                .ok_or("Missing pipeline tile during copy")?;
            batch.push((key, bytes));
            if batch.len() >= 64 {
                writer.put_batch(&batch, None)?;
                batch.clear();
            }
            count += 1;
        }
        if count == 0 || reader.get(key(1, 0, 0))?.is_none() {
            return Err("Incomplete pipeline archive".into());
        }
        if !batch.is_empty() {
            writer.put_batch(&batch, None)?;
        }
        writer.metadata(MANIFEST, &manifest)?;
        Ok(count)
    })();
    let _ = reader.connection.execute_batch("ROLLBACK");
    result
}

fn encoded(id: i64, name: &str, points: Vec<CellPoint>) -> CellFeature {
    CellFeature {
        way_id: id,
        class: 0,
        is_polygon: false,
        name: Some(name.to_owned()),
        points,
        elevations: None,
    }
}
