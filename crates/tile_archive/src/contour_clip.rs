//! Strict XY GeoPackage clipping for durable terrain imports.
use crate::contour_grid::{Bounds, clip_line};
pub fn clip_gpkg(blob: &[u8], bounds: Bounds) -> Result<Option<Vec<u8>>, String> {
    if blob.len() < 8 || &blob[..2] != b"GP" {
        return Err("Invalid contour GeoPackage header".into());
    }
    let envelope = match (blob[3] >> 1) & 7 {
        0 => 0,
        1 => 32,
        2 | 3 => 48,
        4 => 64,
        _ => return Err("Invalid contour envelope".into()),
    };
    let bytes = blob
        .get(8 + envelope..)
        .ok_or("Truncated contour envelope")?;
    let mut cursor = 0;
    let (little, kind) = header(bytes, &mut cursor)?;
    let lines = match kind {
        2 => vec![line(bytes, &mut cursor, little)?],
        5 => {
            let n = word(bytes, &mut cursor, little)? as usize;
            if n > bytes.len().saturating_sub(cursor) / 9 {
                return Err("Invalid contour part count".into());
            }
            let mut lines = Vec::with_capacity(n);
            for _ in 0..n {
                let (le, t) = header(bytes, &mut cursor)?;
                if t != 2 {
                    return Err("Contour child is not an XY LineString".into());
                }
                lines.push(line(bytes, &mut cursor, le)?);
            }
            lines
        }
        _ => return Err("Contour is not an XY line geometry".into()),
    };
    if cursor != bytes.len() {
        return Err("Trailing contour geometry bytes".into());
    }
    let parts: Vec<_> = lines.iter().flat_map(|l| clip_line(l, bounds)).collect();
    if parts.is_empty() {
        return Ok(None);
    }
    Ok(Some(encode(&parts)))
}
pub fn encode(lines: &[Vec<(f64, f64)>]) -> Vec<u8> {
    let mut out = b"GP\0\x01".to_vec();
    out.extend_from_slice(&4326i32.to_le_bytes());
    out.push(1);
    out.extend_from_slice(&5u32.to_le_bytes());
    out.extend_from_slice(&(lines.len() as u32).to_le_bytes());
    for line in lines {
        out.push(1);
        out.extend_from_slice(&2u32.to_le_bytes());
        out.extend_from_slice(&(line.len() as u32).to_le_bytes());
        for &(x, y) in line {
            out.extend_from_slice(&x.to_le_bytes());
            out.extend_from_slice(&y.to_le_bytes());
        }
    }
    out
}
fn header(b: &[u8], p: &mut usize) -> Result<(bool, u32), String> {
    let endian = *b.get(*p).ok_or("Truncated WKB header")?;
    *p += 1;
    if endian > 1 {
        return Err("Invalid WKB byte order".into());
    }
    Ok((endian == 1, word(b, p, endian == 1)?))
}
fn word(b: &[u8], p: &mut usize, le: bool) -> Result<u32, String> {
    let v = b
        .get(*p..*p + 4)
        .ok_or("Truncated WKB word")?
        .try_into()
        .unwrap();
    *p += 4;
    Ok(if le {
        u32::from_le_bytes(v)
    } else {
        u32::from_be_bytes(v)
    })
}
fn line(b: &[u8], p: &mut usize, le: bool) -> Result<Vec<(f64, f64)>, String> {
    let n = word(b, p, le)? as usize;
    if n > b.len().saturating_sub(*p) / 16 {
        return Err("Invalid contour point count".into());
    }
    let mut result = Vec::with_capacity(n);
    for _ in 0..n {
        let x = b[*p..*p + 8].try_into().unwrap();
        let y = b[*p + 8..*p + 16].try_into().unwrap();
        *p += 16;
        let point = if le {
            (f64::from_le_bytes(x), f64::from_le_bytes(y))
        } else {
            (f64::from_be_bytes(x), f64::from_be_bytes(y))
        };
        if !point.0.is_finite() || !point.1.is_finite() {
            return Err("Nonfinite contour coordinate".into());
        }
        result.push(point);
    }
    Ok(result)
}
