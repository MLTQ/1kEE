use crate::osm_ingest::GeoBounds;
use std::path::Path;
use tile_archive::{Key, Reader, contours};

pub fn open(root: Option<&Path>) -> Option<Reader> {
    let mut paths = Vec::new();
    if let Some(derived) = crate::terrain_assets::find_derived_root(root) {
        paths.push(derived.join(tile_archive::FILE_NAME));
    }
    if let Some(root) = root {
        paths.push(root.join(tile_archive::FILE_NAME));
    }
    let path = paths.into_iter().find(|p| p.is_file())?;
    match Reader::open(&path) {
        Ok(reader) => Some(reader),
        Err(e) => {
            eprintln!("[1kEE] {}: {e}", path.display());
            None
        }
    }
}

pub fn vector_cell(
    reader: Option<&Reader>,
    source: &Path,
    tag: [u8; 4],
    lat: i32,
    lon: i32,
    view: GeoBounds,
) -> Option<Vec<cell_format::CellFeature>> {
    let reader = reader?;
    if source.exists() {
        if !tile_archive::vector::prefer_subtiles(
            lat,
            lon,
            [view.min_lat, view.max_lat, view.min_lon, view.max_lon],
        ) && let Ok(bytes) = std::fs::read(source)
            && let Some(features) = cell_format::read::read_single_chunk(&bytes, tag)
        {
            return Some(features);
        }
        let key = format!("vector:{}:{lat}:{lon}", String::from_utf8_lossy(&tag));
        if reader.metadata(&key).ok().flatten()? != contours::fingerprint(source).ok()? {
            return None;
        }
    }
    match tile_archive::vector::read_cell(
        reader,
        tag,
        lat,
        lon,
        [view.min_lat, view.max_lat, view.min_lon, view.max_lon],
    ) {
        Ok(value) => value,
        Err(e) => {
            eprintln!("[1kEE] Archive vector fallback ({lat},{lon}): {e}");
            None
        }
    }
}

pub struct ContourArchive {
    reader: Reader,
    body: i32,
}

impl ContourArchive {
    pub fn open(source: &Path) -> Option<Self> {
        let body = match source.file_name()?.to_str()? {
            "srtm_focus_cache.sqlite" => 0,
            "lunar_focus_cache.sqlite" => 1,
            "mars_ctx_cache.sqlite" => 2,
            _ => return None,
        };
        let path = source.parent()?.parent()?.join(tile_archive::FILE_NAME);
        if !path.is_file() {
            return None;
        }
        let reader = Reader::open(&path).ok()?;
        if reader
            .metadata(&format!("contours:{body}"))
            .ok()
            .flatten()?
            != contours::fingerprint(source).ok()?
        {
            return None;
        }
        Some(Self { reader, body })
    }

    pub fn get(&self, level: i32, y: i32, x: i32) -> Option<Vec<u8>> {
        match self.reader.get(Key {
            body: self.body,
            layer: *b"CNTR",
            grid: 2,
            level,
            y,
            x,
        }) {
            Ok(value) => value,
            Err(e) => {
                eprintln!("[1kEE] Packed contour fallback: {e}");
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn changed_source_cell_falls_back_but_removed_sources_allow_archive_reads() {
        let root = std::env::temp_dir().join(format!(
            "1kee-archive-source-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let source = root.join("road.1kc");
        std::fs::write(&source, b"original").unwrap();
        let path = root.join("world.1ka");
        let mut writer = tile_archive::Writer::create(&path).unwrap();
        tile_archive::vector::pack_cell(&mut writer, *b"ROAD", 0, 0, &[]).unwrap();
        writer
            .metadata("vector:ROAD:0:0", &contours::fingerprint(&source).unwrap())
            .unwrap();
        writer.finish().unwrap();
        let reader = Reader::open(&path).unwrap();
        let bounds = GeoBounds {
            min_lat: 0.0,
            max_lat: 0.1,
            min_lon: 0.0,
            max_lon: 0.1,
        };
        assert!(
            vector_cell(Some(&reader), &source, *b"ROAD", 0, 0, bounds)
                .unwrap()
                .is_empty()
        );
        std::fs::write(&source, b"changed and longer").unwrap();
        assert!(vector_cell(Some(&reader), &source, *b"ROAD", 0, 0, bounds).is_none());
        std::fs::remove_file(&source).unwrap();
        assert!(vector_cell(Some(&reader), &source, *b"ROAD", 0, 0, bounds).is_some());
        drop(reader);
        std::fs::remove_dir_all(root).unwrap();
    }
}
