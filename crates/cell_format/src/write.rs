use crate::{CellFeature, FLAG_HAS_ELEVATION, FLAG_HAS_NAME, FLAG_IS_POLYGON, MAGIC, VERSION};

/// Serialise a complete `.1kc` file to a byte buffer.
///
/// `cell_lat` and `cell_lon` are the integer floor coordinates of the cell
/// (matching the filename convention).  `chunks` is a slice of `(tag,
/// features)` pairs; each pair becomes one chunk in the output, in order.
pub fn write_cell(cell_lat: i16, cell_lon: i16, chunks: &[([u8; 4], &[CellFeature])]) -> Vec<u8> {
    let mut capacity = 10usize;
    for (_, features) in chunks {
        capacity += 12; // tag (4) + length (4) + feature_count (4)
        for f in *features {
            capacity += feature_encoded_size(f);
        }
    }
    let mut buf = Vec::with_capacity(capacity);
    write_header(&mut buf, cell_lat, cell_lon);
    for (tag, features) in chunks {
        buf.extend_from_slice(tag);
        let len_pos = buf.len();
        buf.extend_from_slice(&0u32.to_le_bytes());
        let start = buf.len();
        buf.extend_from_slice(&(features.len() as u32).to_le_bytes());
        for f in *features {
            encode_feature_into(&mut buf, f);
        }
        let len = (buf.len() - start) as u32;
        buf[len_pos..len_pos + 4].copy_from_slice(&len.to_le_bytes());
    }
    buf
}

/// Zero-clone variant of [`write_cell`] accepting slices of feature references.
pub fn write_cell_refs(
    cell_lat: i16,
    cell_lon: i16,
    chunks: &[([u8; 4], &[&CellFeature])],
) -> Vec<u8> {
    let mut capacity = 10usize;
    for (_, features) in chunks {
        capacity += 12;
        for f in *features {
            capacity += feature_encoded_size(f);
        }
    }
    let mut buf = Vec::with_capacity(capacity);
    write_header(&mut buf, cell_lat, cell_lon);
    for (tag, features) in chunks {
        buf.extend_from_slice(tag);
        let len_pos = buf.len();
        buf.extend_from_slice(&0u32.to_le_bytes());
        let start = buf.len();
        buf.extend_from_slice(&(features.len() as u32).to_le_bytes());
        for f in *features {
            encode_feature_into(&mut buf, f);
        }
        let len = (buf.len() - start) as u32;
        buf[len_pos..len_pos + 4].copy_from_slice(&len.to_le_bytes());
    }
    buf
}

#[inline]
fn write_header(buf: &mut Vec<u8>, cell_lat: i16, cell_lon: i16) {
    buf.extend_from_slice(&MAGIC);
    buf.push(VERSION);
    buf.extend_from_slice(&cell_lat.to_le_bytes());
    buf.extend_from_slice(&cell_lon.to_le_bytes());
    buf.push(0u8); // reserved
}

#[inline]
fn feature_encoded_size(f: &CellFeature) -> usize {
    let mut size = 8 + 1 + 1 + 2 + 4; // way_id, class, flags, name_len, point_count
    if let Some(name) = &f.name {
        size += name.len();
    }
    let point_bytes = if f.elevations.is_some() { 12 } else { 8 };
    size + f.points.len() * point_bytes
}

#[inline]
fn encode_feature_into(buf: &mut Vec<u8>, f: &CellFeature) {
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
                let mut rec = [0u8; 8];
                rec[0..4].copy_from_slice(&pt.lon.to_le_bytes());
                rec[4..8].copy_from_slice(&pt.lat.to_le_bytes());
                buf.extend_from_slice(&rec);
            }
        }
        Some(elevs) => {
            for (pt, &elev) in f.points.iter().zip(elevs.iter()) {
                let mut rec = [0u8; 12];
                rec[0..4].copy_from_slice(&pt.lon.to_le_bytes());
                rec[4..8].copy_from_slice(&pt.lat.to_le_bytes());
                rec[8..12].copy_from_slice(&elev.to_le_bytes());
                buf.extend_from_slice(&rec);
            }
        }
    }
}
