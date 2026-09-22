use std::path::Path;

pub fn encode(rows: &[(f32, Vec<u8>)]) -> Result<Vec<u8>, String> {
    let mut bytes = b"CTF1".to_vec();
    bytes.extend_from_slice(
        &u32::try_from(rows.len())
            .map_err(|e| e.to_string())?
            .to_le_bytes(),
    );
    for (elevation, geometry) in rows {
        let lines =
            crate::gpkg::parse_gpkg_lines(geometry, |lon, lat| cell_format::CellPoint { lon, lat });
        let mut packed = Vec::new();
        packed.extend_from_slice(&(lines.len() as u32).to_le_bytes());
        for line in lines {
            packed.extend_from_slice(&(line.len() as u32).to_le_bytes());
            for p in line {
                packed.extend_from_slice(&p.lon.to_le_bytes());
                packed.extend_from_slice(&p.lat.to_le_bytes());
            }
        }
        if !elevation.is_finite()
            || bytes.len().saturating_add(8).saturating_add(packed.len()) > crate::MAX_PAYLOAD
        {
            return Err("Invalid/oversized contour tile".into());
        }
        bytes.extend_from_slice(&elevation.to_le_bytes());
        bytes.extend_from_slice(&(packed.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&packed);
    }
    Ok(bytes)
}

pub fn decode(bytes: &[u8]) -> Result<Vec<(f32, &[u8])>, String> {
    let invalid = || "Invalid packed contour tile".to_owned();
    if bytes.len() < 8 || bytes.len() > crate::MAX_PAYLOAD || &bytes[..4] != b"CTF1" {
        return Err(invalid());
    }
    let count = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
    if count > (bytes.len() - 8) / 8 {
        return Err(invalid());
    }
    let mut rows = Vec::with_capacity(count);
    let mut pos = 8;
    for _ in 0..count {
        let header = bytes.get(pos..pos + 8).ok_or_else(invalid)?;
        let elevation = f32::from_le_bytes(header[..4].try_into().unwrap());
        let length = u32::from_le_bytes(header[4..].try_into().unwrap()) as usize;
        pos += 8;
        if !elevation.is_finite() {
            return Err(invalid());
        }
        let geometry = bytes
            .get(pos..pos.checked_add(length).ok_or_else(invalid)?)
            .ok_or_else(invalid)?;
        validate_lines(geometry)?;
        pos += length;
        rows.push((elevation, geometry));
    }
    if pos != bytes.len() {
        return Err(invalid());
    }
    Ok(rows)
}

fn word(bytes: &[u8], pos: &mut usize) -> Result<usize, String> {
    let b = bytes
        .get(*pos..pos.saturating_add(4))
        .ok_or("Invalid packed line count")?;
    *pos += 4;
    Ok(u32::from_le_bytes(b.try_into().unwrap()) as usize)
}

fn validate_lines(bytes: &[u8]) -> Result<(), String> {
    let mut pos = 0;
    let lines = word(bytes, &mut pos)?;
    if lines > bytes.len().saturating_sub(pos) / 4 {
        return Err("Invalid packed part count".into());
    }
    for _ in 0..lines {
        let count = word(bytes, &mut pos)?;
        if count > bytes.len().saturating_sub(pos) / 8 {
            return Err("Invalid packed point count".into());
        }
        pos += count * 8;
    }
    if pos != bytes.len() {
        return Err("Trailing packed geometry bytes".into());
    }
    Ok(())
}

pub fn decode_lines<T>(
    bytes: &[u8],
    mut point: impl FnMut(f32, f32) -> T,
) -> Result<Vec<Vec<T>>, String> {
    validate_lines(bytes)?;
    let mut pos = 0;
    let count = word(bytes, &mut pos)?;
    let mut lines = Vec::with_capacity(count);
    for _ in 0..count {
        let count = word(bytes, &mut pos)?;
        let mut points = Vec::with_capacity(count);
        for _ in 0..count {
            let lon = f32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap());
            let lat = f32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap());
            pos += 8;
            points.push(point(lon, lat));
        }
        lines.push(points);
    }
    Ok(lines)
}

pub fn fingerprint(path: &Path) -> Result<String, String> {
    use std::os::unix::fs::MetadataExt;
    let stamp = |p: &Path| -> Result<String, String> {
        let m = std::fs::metadata(p).map_err(|e| e.to_string())?;
        Ok(format!(
            "{}:{}:{}:{}:{}",
            m.dev(),
            m.ino(),
            m.len(),
            m.mtime(),
            m.mtime_nsec()
        ))
    };
    let wal = path.with_file_name(format!(
        "{}-wal",
        path.file_name().ok_or("No filename")?.to_string_lossy()
    ));
    Ok(format!(
        "{}|{}",
        stamp(path)?,
        if wal.exists() && std::fs::metadata(&wal).map_err(|e| e.to_string())?.len() > 0 {
            stamp(&wal)?
        } else {
            "none".into()
        }
    ))
}
