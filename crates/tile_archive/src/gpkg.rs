pub fn parse_gpkg_lines<T>(blob: &[u8], mut point: impl FnMut(f32, f32) -> T) -> Vec<Vec<T>> {
    if blob.len() < 8 || &blob[0..2] != b"GP" {
        return Vec::new();
    }

    let flags = blob[3];
    let envelope_indicator = (flags >> 1) & 0b111;
    let envelope_len = match envelope_indicator {
        0 => 0,
        1 => 32,
        2 | 3 => 48,
        4 => 64,
        _ => 0,
    };
    let header_len = 8 + envelope_len;
    if blob.len() <= header_len {
        return Vec::new();
    }

    parse_wkb_geometry(&blob[header_len..], &mut point).unwrap_or_default()
}

pub fn parse_wkb_geometry<T>(
    wkb: &[u8],
    mut point: impl FnMut(f32, f32) -> T,
) -> Option<Vec<Vec<T>>> {
    let mut cursor = 0usize;
    let endian = *wkb.get(cursor)?;
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
