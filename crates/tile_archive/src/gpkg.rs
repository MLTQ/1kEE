pub fn parse_gpkg_lines<T>(blob: &[u8], point: impl FnMut(f32, f32) -> T) -> Vec<Vec<T>> {
    try_parse_gpkg_lines(blob, point).unwrap_or_default()
}

/// Preserve the difference between valid empty geometry and damaged bytes.
pub fn try_parse_gpkg_lines<T>(blob: &[u8], mut point: impl FnMut(f32, f32) -> T) -> Option<Vec<Vec<T>>> {
    if blob.len() < 8 || &blob[0..2] != b"GP" || blob[2] != 0 { return None; }
    let envelope_len = match (blob[3] >> 1) & 7 {
        0 => 0, 1 => 32, 2 | 3 => 48, 4 => 64, _ => return None,
    };
    let wkb = blob.get(8 + envelope_len..)?;
    if !matches!(wkb.first(), Some(0 | 1)) { return None; }
    let mut finite = true;
    let lines = parse_wkb_geometry(wkb, |x, y| {
        finite &= x.is_finite() && y.is_finite();
        point(x, y)
    })?;
    finite.then_some(lines)
}

pub fn parse_wkb_geometry<T>(
    wkb: &[u8],
    mut point: impl FnMut(f32, f32) -> T,
) -> Option<Vec<Vec<T>>> {
    let mut cursor = 0usize;
    let endian = *wkb.get(cursor)?;
    if endian > 1 { return None; }
    cursor += 1;
    let little = endian == 1;
    let geom_type = read_u32(wkb, &mut cursor, little)?;
    let base_type = geom_type % 1000;

    match base_type {
        2 => Some(vec![parse_linestring(
            wkb,
            &mut cursor,
            little,
            &mut point,
        )?]),
        5 => {
            let count = read_u32(wkb, &mut cursor, little)? as usize;
            // Every direct child has at least one byte of endian marker, four
            // bytes of geometry type, and four bytes of point count. Validate
            // that minimum before reserving so a corrupt count cannot trigger
            // an enormous allocation.
            if count > wkb.len().checked_sub(cursor)? / 9 {
                return None;
            }
            let mut lines = Vec::with_capacity(count);
            for _ in 0..count {
                lines.push(parse_wkb_linestring(wkb, &mut cursor, &mut point)?);
            }
            Some(lines)
        }
        _ => None,
    }
}

/// Parse a child of a WKB `MultiLineString`. WKB requires every child to be a
/// direct `LineString`, so deliberately rejecting nested collections keeps an
/// untrusted/corrupt cache blob from recursing an unnamed loader thread into a
/// stack overflow.
fn parse_wkb_linestring<T>(
    wkb: &[u8],
    cursor: &mut usize,
    point: &mut impl FnMut(f32, f32) -> T,
) -> Option<Vec<T>> {
    let endian = *wkb.get(*cursor)?;
    if endian > 1 { return None; }
    *cursor += 1;
    let little = endian == 1;
    let geom_type = read_u32(wkb, cursor, little)?;
    if geom_type % 1000 != 2 {
        return None;
    }
    parse_linestring(wkb, cursor, little, point)
}

fn parse_linestring<T>(
    wkb: &[u8],
    cursor: &mut usize,
    little: bool,
    point: &mut impl FnMut(f32, f32) -> T,
) -> Option<Vec<T>> {
    let count = read_u32(wkb, cursor, little)? as usize;
    // Each XY point occupies two f64 values. Check before reserving to reject
    // malformed count fields without a large allocation attempt.
    let end = cursor.checked_add(count.checked_mul(16)?)?;
    let coordinates = wkb.get(*cursor..end)?.as_chunks::<16>().0;
    *cursor = end;
    let mut points = Vec::with_capacity(count);
    // Test byte order once per line. Each exact chunk is one XY pair.
    if little {
        points.extend(coordinates.iter().map(|pair| {
            let (x, y) = pair.split_at(8);
            point(
                f64::from_le_bytes(x.try_into().unwrap()) as f32,
                f64::from_le_bytes(y.try_into().unwrap()) as f32,
            )
        }));
    } else {
        points.extend(coordinates.iter().map(|pair| {
            let (x, y) = pair.split_at(8);
            point(
                f64::from_be_bytes(x.try_into().unwrap()) as f32,
                f64::from_be_bytes(y.try_into().unwrap()) as f32,
            )
        }));
    }
    Some(points)
}

/// Zero-allocation validator equivalent to `try_parse_gpkg_lines(blob, |_, _| ()).is_some()`.
pub fn validate_gpkg_lines(blob: &[u8]) -> bool {
    if blob.len() < 8 || &blob[0..2] != b"GP" || blob[2] != 0 {
        return false;
    }
    let envelope_len = match (blob[3] >> 1) & 7 {
        0 => 0,
        1 => 32,
        2 | 3 => 48,
        4 => 64,
        _ => return false,
    };
    let Some(wkb) = blob.get(8 + envelope_len..) else {
        return false;
    };
    let mut cursor = 0usize;
    let Some(&endian) = wkb.get(cursor) else {
        return false;
    };
    if endian > 1 {
        return false;
    }
    cursor += 1;
    let little = endian == 1;
    let Some(geom_type) = read_u32(wkb, &mut cursor, little) else {
        return false;
    };
    match geom_type % 1000 {
        2 => validate_linestring(wkb, &mut cursor, little),
        5 => {
            let Some(count) = read_u32(wkb, &mut cursor, little).map(|n| n as usize) else {
                return false;
            };
            if wkb.len().checked_sub(cursor).is_none_or(|rem| count > rem / 9) {
                return false;
            }
            for _ in 0..count {
                let Some(&e) = wkb.get(cursor) else {
                    return false;
                };
                if e > 1 {
                    return false;
                }
                cursor += 1;
                let lit = e == 1;
                let Some(gt) = read_u32(wkb, &mut cursor, lit) else {
                    return false;
                };
                if gt % 1000 != 2 || !validate_linestring(wkb, &mut cursor, lit) {
                    return false;
                }
            }
            true
        }
        _ => false,
    }
}

fn validate_linestring(wkb: &[u8], cursor: &mut usize, little: bool) -> bool {
    let Some(count) = read_u32(wkb, cursor, little).map(|n| n as usize) else {
        return false;
    };
    let Some(end) = count.checked_mul(16).and_then(|n| cursor.checked_add(n)) else {
        return false;
    };
    let Some(slice) = wkb.get(*cursor..end) else {
        return false;
    };
    *cursor = end;
    let pairs = slice.as_chunks::<16>().0;
    if little {
        pairs.iter().all(|pair| {
            let x = f64::from_le_bytes(pair[0..8].try_into().unwrap()) as f32;
            let y = f64::from_le_bytes(pair[8..16].try_into().unwrap()) as f32;
            x.is_finite() && y.is_finite()
        })
    } else {
        pairs.iter().all(|pair| {
            let x = f64::from_be_bytes(pair[0..8].try_into().unwrap()) as f32;
            let y = f64::from_be_bytes(pair[8..16].try_into().unwrap()) as f32;
            x.is_finite() && y.is_finite()
        })
    }
}

/// Stream a GeoPackage WKB LineString/MultiLineString directly into CTF1 geometry
/// payload bytes (part_count + per-part (point_count, f32 lon, f32 lat)) with
/// zero intermediate `Vec<Vec<...>>` allocations.
pub fn stream_gpkg_to_ctf1(blob: &[u8], out: &mut Vec<u8>) -> bool {
    if blob.len() < 8 || &blob[0..2] != b"GP" || blob[2] != 0 {
        return false;
    }
    let envelope_len = match (blob[3] >> 1) & 7 {
        0 => 0,
        1 => 32,
        2 | 3 => 48,
        4 => 64,
        _ => return false,
    };
    let Some(wkb) = blob.get(8 + envelope_len..) else {
        return false;
    };
    let mut cursor = 0usize;
    let Some(&endian) = wkb.get(cursor) else {
        return false;
    };
    if endian > 1 {
        return false;
    }
    cursor += 1;
    let little = endian == 1;
    let Some(geom_type) = read_u32(wkb, &mut cursor, little) else {
        return false;
    };
    let start_len = out.len();
    match geom_type % 1000 {
        2 => {
            out.extend_from_slice(&1u32.to_le_bytes());
            if !stream_linestring_to_ctf1(wkb, &mut cursor, little, out) {
                out.truncate(start_len);
                return false;
            }
            true
        }
        5 => {
            let Some(count) = read_u32(wkb, &mut cursor, little).map(|n| n as usize) else {
                return false;
            };
            if wkb.len().checked_sub(cursor).is_none_or(|rem| count > rem / 9) {
                return false;
            }
            out.extend_from_slice(&(count as u32).to_le_bytes());
            for _ in 0..count {
                let Some(&e) = wkb.get(cursor) else {
                    out.truncate(start_len);
                    return false;
                };
                if e > 1 {
                    out.truncate(start_len);
                    return false;
                }
                cursor += 1;
                let lit = e == 1;
                let Some(gt) = read_u32(wkb, &mut cursor, lit) else {
                    out.truncate(start_len);
                    return false;
                };
                if gt % 1000 != 2 || !stream_linestring_to_ctf1(wkb, &mut cursor, lit, out) {
                    out.truncate(start_len);
                    return false;
                }
            }
            true
        }
        _ => false,
    }
}

fn stream_linestring_to_ctf1(
    wkb: &[u8],
    cursor: &mut usize,
    little: bool,
    out: &mut Vec<u8>,
) -> bool {
    let Some(count) = read_u32(wkb, cursor, little).map(|n| n as usize) else {
        return false;
    };
    let Some(end) = count.checked_mul(16).and_then(|n| cursor.checked_add(n)) else {
        return false;
    };
    let Some(slice) = wkb.get(*cursor..end) else {
        return false;
    };
    *cursor = end;
    out.reserve(4 + count * 8);
    out.extend_from_slice(&(count as u32).to_le_bytes());
    let pairs = slice.as_chunks::<16>().0;
    if little {
        for pair in pairs {
            let lon = f64::from_le_bytes(pair[0..8].try_into().unwrap()) as f32;
            let lat = f64::from_le_bytes(pair[8..16].try_into().unwrap()) as f32;
            if !lon.is_finite() || !lat.is_finite() {
                return false;
            }
            let mut rec = [0u8; 8];
            rec[0..4].copy_from_slice(&lon.to_le_bytes());
            rec[4..8].copy_from_slice(&lat.to_le_bytes());
            out.extend_from_slice(&rec);
        }
    } else {
        for pair in pairs {
            let lon = f64::from_be_bytes(pair[0..8].try_into().unwrap()) as f32;
            let lat = f64::from_be_bytes(pair[8..16].try_into().unwrap()) as f32;
            if !lon.is_finite() || !lat.is_finite() {
                return false;
            }
            let mut rec = [0u8; 8];
            rec[0..4].copy_from_slice(&lon.to_le_bytes());
            rec[4..8].copy_from_slice(&lat.to_le_bytes());
            out.extend_from_slice(&rec);
        }
    }
    true
}

fn read_u32(bytes: &[u8], cursor: &mut usize, little: bool) -> Option<u32> {
    let end = (*cursor).checked_add(4)?;
    let slice = bytes.get(*cursor..end)?;
    *cursor = end;
    Some(if little {
        u32::from_le_bytes(slice.try_into().ok()?)
    } else {
        u32::from_be_bytes(slice.try_into().ok()?)
    })
}

#[cfg(test)]
mod recovery_validation_tests {
    use super::*;
    #[test]
    fn checked_parser_distinguishes_empty_from_corrupt_and_nonfinite() {
        let empty = crate::contour_clip::encode(&[]);
        assert_eq!(try_parse_gpkg_lines(&empty, |x,y| (x,y)),Some(vec![]));
        assert!(try_parse_gpkg_lines(b"broken", |_,_| ()).is_none());
        let nan = crate::contour_clip::encode(&[vec![(f64::NAN,1.0),(1.0,2.0)]]);
        assert!(try_parse_gpkg_lines(&nan, |_,_| ()).is_none());
        let valid = crate::contour_clip::encode(&[vec![(1.0,2.0),(3.0,4.0)]]);
        assert!(try_parse_gpkg_lines(&valid[..valid.len()-1], |_,_| ()).is_none());
        assert_eq!(try_parse_gpkg_lines(&valid, |x,y| (x,y)),Some(vec![vec![(1.0,2.0),(3.0,4.0)]]));
    }
}
