use crate::{CellFeature, CellPoint, FLAG_HAS_ELEVATION, FLAG_HAS_NAME, FLAG_IS_POLYGON, MAGIC, VERSION};

/// Validate the file header and return the byte offset of the first chunk.
///
/// Returns `None` if the magic bytes are wrong, the version is unsupported,
/// or the data is too short to hold a valid header.
pub fn check_header(data: &[u8]) -> Option<usize> {
    if data.len() < 10 {
        return None;
    }
    if data[0..4] != MAGIC {
        return None;
    }
    if data[4] != VERSION {
        return None;
    }
    Some(10)
}

/// Read all chunks from `data` into `(tag, features)` pairs in file order.
///
/// Unknown chunk tags are decoded as empty feature lists and still included in
/// the output — callers that need only one tag can filter with
/// [`read_single_chunk`].  Returns `None` only if the header is invalid.
pub fn read_chunks(data: &[u8]) -> Option<Vec<([u8; 4], Vec<CellFeature>)>> {
    let mut pos = check_header(data)?;
    let mut chunks = Vec::new();

    while pos + 8 <= data.len() {
        let tag: [u8; 4] = data[pos..pos + 4].try_into().unwrap();
        let length = u32::from_le_bytes(data[pos + 4..pos + 8].try_into().unwrap()) as usize;
        pos += 8;

        if pos + length > data.len() {
            break; // truncated chunk — stop gracefully
        }
        let payload = &data[pos..pos + length];
        pos += length;

        let features = decode_features(payload).unwrap_or_default();
        chunks.push((tag, features));
    }

    Some(chunks)
}

/// Read the first chunk matching `tag` and return its features.
///
/// Returns `None` if the header is invalid or no matching chunk exists.
pub fn read_single_chunk(data: &[u8], tag: [u8; 4]) -> Option<Vec<CellFeature>> {
    read_chunks(data)?
        .into_iter()
        .find(|(t, _)| *t == tag)
        .map(|(_, f)| f)
}

// ── Internal decoder ─────────────────────────────────────────────────────────

fn decode_features(data: &[u8]) -> Option<Vec<CellFeature>> {
    if data.len() < 4 {
        return None;
    }
    let count = u32::from_le_bytes(data[0..4].try_into().unwrap()) as usize;
    let mut features = Vec::with_capacity(count);
    let mut pos = 4usize;

    for _ in 0..count {
        // way_id: 8 bytes, class: 1, flags: 1  →  minimum 10 bytes before name_len
        if pos + 10 > data.len() {
            return None;
        }
        let way_id = i64::from_le_bytes(data[pos..pos + 8].try_into().unwrap());
        pos += 8;

        let class = data[pos];
        pos += 1;
        let flags = data[pos];
        pos += 1;

        let is_polygon = flags & FLAG_IS_POLYGON != 0;
        let has_name = flags & FLAG_HAS_NAME != 0;
        let has_elevation = flags & FLAG_HAS_ELEVATION != 0;

        // name_len (always present; zero when has_name = 0).
        if pos + 2 > data.len() {
            return None;
        }
        let name_len = u16::from_le_bytes(data[pos..pos + 2].try_into().unwrap()) as usize;
        pos += 2;

        let name = if has_name && name_len > 0 {
            if pos + name_len > data.len() {
                return None;
            }
            let s = std::str::from_utf8(&data[pos..pos + name_len]).ok()?.to_owned();
            pos += name_len;
            Some(s)
        } else {
            pos += name_len; // skip any stray bytes defensively
            None
        };

        // point_count.
        if pos + 4 > data.len() {
            return None;
        }
        let point_count = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
        pos += 4;

        // Points (8 or 12 bytes each).
        let point_stride = if has_elevation { 12 } else { 8 };
        if pos + point_count * point_stride > data.len() {
            return None;
        }

        let mut points = Vec::with_capacity(point_count);
        let mut elevations: Option<Vec<f32>> = if has_elevation {
            Some(Vec::with_capacity(point_count))
        } else {
            None
        };

        for _ in 0..point_count {
            let lon = f32::from_le_bytes(data[pos..pos + 4].try_into().unwrap());
            let lat = f32::from_le_bytes(data[pos + 4..pos + 8].try_into().unwrap());
            points.push(CellPoint { lon, lat });

            if has_elevation {
                let elev = f32::from_le_bytes(data[pos + 8..pos + 12].try_into().unwrap());
                elevations.as_mut().unwrap().push(elev);
                pos += 12;
            } else {
                pos += 8;
            }
        }

        features.push(CellFeature {
            way_id,
            class,
            is_polygon,
            name,
            points,
            elevations,
        });
    }

    Some(features)
}
