use crate::{CellFeature, FLAG_HAS_ELEVATION, FLAG_HAS_NAME, FLAG_IS_POLYGON, MAGIC, VERSION};

/// Serialise a complete `.1kc` file to a byte buffer.
///
/// `cell_lat` and `cell_lon` are the integer floor coordinates of the cell
/// (matching the filename convention).  `chunks` is a slice of `(tag,
/// features)` pairs; each pair becomes one chunk in the output, in order.
pub fn write_cell(
    cell_lat: i16,
    cell_lon: i16,
    chunks: &[([u8; 4], &[CellFeature])],
) -> Vec<u8> {
    // Pre-calculate capacity to avoid repeated reallocations.
    let mut capacity = 10; // header
    for (_, features) in chunks {
        capacity += 8; // tag (4) + length (4)
        capacity += 4; // feature_count
        for f in *features {
            capacity += 8 + 1 + 1 + 2 + 4; // way_id, class, flags, name_len, point_count
            if let Some(name) = &f.name {
                capacity += name.len();
            }
            let point_bytes = if f.elevations.is_some() { 12 } else { 8 };
            capacity += f.points.len() * point_bytes;
        }
    }

    let mut buf = Vec::with_capacity(capacity);

    // File header.
    buf.extend_from_slice(&MAGIC);
    buf.push(VERSION);
    buf.extend_from_slice(&cell_lat.to_le_bytes());
    buf.extend_from_slice(&cell_lon.to_le_bytes());
    buf.push(0u8); // reserved

    // Chunks.
    for (tag, features) in chunks {
        let payload = encode_features(features);
        buf.extend_from_slice(tag);
        buf.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        buf.extend(payload);
    }

    buf
}

fn encode_features(features: &[CellFeature]) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(&(features.len() as u32).to_le_bytes());

    for f in features {
        // way_id (or relation_id for ADMN chunks).
        buf.extend_from_slice(&f.way_id.to_le_bytes());

        // class enum byte.
        buf.push(f.class);

        // flags byte.
        let mut flags = 0u8;
        if f.is_polygon {
            flags |= FLAG_IS_POLYGON;
        }
        if f.name.is_some() {
            flags |= FLAG_HAS_NAME;
        }
        if f.elevations.is_some() {
            flags |= FLAG_HAS_ELEVATION;
        }
        buf.push(flags);

        // Optional name (length-prefixed UTF-8).
        match &f.name {
            Some(name) => {
                let bytes = name.as_bytes();
                buf.extend_from_slice(&(bytes.len() as u16).to_le_bytes());
                buf.extend_from_slice(bytes);
            }
            None => buf.extend_from_slice(&0u16.to_le_bytes()),
        }

        // Point array.
        buf.extend_from_slice(&(f.points.len() as u32).to_le_bytes());
        match &f.elevations {
            None => {
                for pt in &f.points {
                    buf.extend_from_slice(&pt.lon.to_le_bytes());
                    buf.extend_from_slice(&pt.lat.to_le_bytes());
                }
            }
            Some(elevs) => {
                for (pt, &elev) in f.points.iter().zip(elevs.iter()) {
                    buf.extend_from_slice(&pt.lon.to_le_bytes());
                    buf.extend_from_slice(&pt.lat.to_le_bytes());
                    buf.extend_from_slice(&elev.to_le_bytes());
                }
            }
        }
    }

    buf
}
