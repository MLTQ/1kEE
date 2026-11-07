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
    if count > wkb.len().checked_sub(*cursor)? / 16 {
        return None;
    }
    let mut points = Vec::with_capacity(count);
    for _ in 0..count {
        let lon = read_f64(wkb, cursor, little)? as f32;
        let lat = read_f64(wkb, cursor, little)? as f32;
        points.push(point(lon, lat));
    }
    Some(points)
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

fn read_f64(bytes: &[u8], cursor: &mut usize, little: bool) -> Option<f64> {
    let end = (*cursor).checked_add(8)?;
    let slice = bytes.get(*cursor..end)?;
    *cursor = end;
    Some(if little {
        f64::from_le_bytes(slice.try_into().ok()?)
    } else {
        f64::from_be_bytes(slice.try_into().ok()?)
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
