//! Windowed GeoTIFF decoding and newest-measured-pixel fusion, without Python/GDAL.
use rusqlite::{Connection, OpenFlags, params};
use std::{
    collections::HashMap,
    fs::File,
    io::BufReader,
    path::{Path, PathBuf},
};
use tiff::{
    decoder::{Decoder, DecodingResult},
    tags::Tag,
};

pub const SIZE: usize = 512;
pub type Credit = [String; 3];

#[derive(Debug)]
pub struct Rendered {
    pub rgba: Vec<u8>,
    pub credits: Vec<Credit>,
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn checked_path(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let relative = Path::new(relative);
    if relative.is_absolute()
        || relative.components().any(|c| {
            !matches!(
                c,
                std::path::Component::Normal(_) | std::path::Component::CurDir
            )
        })
    {
        return Err("Archive receipt escapes the lidar root".into());
    }
    let path = root.join(relative);
    if path.exists()
        && !path
            .canonicalize()
            .map_err(err)?
            .starts_with(root.canonicalize().map_err(err)?)
    {
        return Err("Archive receipt resolves outside the lidar root".into());
    }
    Ok(path)
}

/// Decode only TIFF chunks sampled by this view; mean is the third v1 band.
pub fn read_mean(path: &Path, [w, s, e, n]: [f64; 4]) -> Result<Vec<f32>, String> {
    if ![w, s, e, n].iter().all(|v| v.is_finite()) || w >= e || s >= n {
        return Err("Invalid lidar view bounds".into());
    }
    let mut decoder = Decoder::new(BufReader::new(File::open(path).map_err(err)?)).map_err(err)?;
    let (width, height) = decoder.dimensions().map_err(err)?;
    let scale = decoder
        .get_tag_f64_vec(Tag::ModelPixelScaleTag)
        .map_err(err)?;
    let tie = decoder
        .get_tag_f64_vec(Tag::ModelTiepointTag)
        .map_err(err)?;
    let samples = decoder.get_tag_u32(Tag::SamplesPerPixel).map_err(err)?;
    let keys = decoder
        .get_tag_u16_vec(Tag::GeoKeyDirectoryTag)
        .map_err(err)?;
    if width == 0
        || height == 0
        || samples != 4
        || scale.len() < 2
        || tie.len() < 6
        || scale[0] <= 0.
        || scale[1] <= 0.
        || !scale[..2].iter().chain(&tie[..6]).all(|v| v.is_finite())
        || width as u64 * height as u64 > 16_000_000
        || !keys
            .get(4..)
            .unwrap_or_default()
            .as_chunks::<4>()
            .0
            .contains(&[2048, 0, 1, 4326])
    {
        return Err("Unsupported numeric lidar raster geometry/CRS/bands".into());
    }
    let planar = decoder.get_tag_u32(Tag::PlanarConfiguration).unwrap_or(1);
    if planar != 1 && planar != 2 {
        return Err("Unsupported raster sample layout".into());
    }
    let (cw, ch) = decoder.chunk_dimensions();
    if cw == 0 || ch == 0 || cw as u64 * ch as u64 > 16_000_000 {
        return Err("Invalid raster chunk size".into());
    }
    let columns = width.div_ceil(cw);
    let planes = columns * height.div_ceil(ch);
    let origin_x = tie[3] - tie[0] * scale[0];
    let origin_y = tie[4] + tie[1] * scale[1];
    let nodata = decoder
        .get_tag_ascii_string(Tag::GdalNodata)
        .ok()
        .and_then(|s| s.trim_end_matches('\0').parse::<f32>().ok());
    let mut chunks: HashMap<u32, Vec<(usize, usize, usize)>> = HashMap::new();
    for y in 0..SIZE {
        let lat = n - (y as f64 + 0.5) * (n - s) / SIZE as f64;
        let row = ((origin_y - lat) / scale[1]).floor();
        if row < 0. || row >= height as f64 {
            continue;
        }
        for x in 0..SIZE {
            let lon = w + (x as f64 + 0.5) * (e - w) / SIZE as f64;
            let col = ((lon - origin_x) / scale[0]).floor();
            if col < 0. || col >= width as f64 {
                continue;
            }
            let (col, row) = (col as u32, row as u32);
            let chunk = row / ch * columns + col / cw + if planar == 2 { 2 * planes } else { 0 };
            chunks.entry(chunk).or_default().push((
                y * SIZE + x,
                (col % cw) as usize,
                (row % ch) as usize,
            ));
        }
    }
    let mut result = vec![f32::NAN; SIZE * SIZE];
    for (chunk, pixels) in chunks {
        let (chunk_width, _) = decoder.chunk_data_dimensions(chunk);
        let DecodingResult::F32(data) = decoder.read_chunk(chunk).map_err(err)? else {
            return Err("Numeric lidar raster must contain float32 samples".into());
        };
        for (dest, x, y) in pixels {
            let pixel = y * chunk_width as usize + x;
            let index = if planar == 2 { pixel } else { pixel * 4 + 2 };
            let value = *data.get(index).ok_or("Truncated numeric raster chunk")?;
            if value.is_finite() && nodata != Some(value) {
                result[dest] = value;
            }
        }
    }
    Ok(result)
}

pub fn shade(values: &[f32], [w, s, e, n]: [f64; 4]) -> Vec<u8> {
    const LEVELS: [f64; 10] = [-150., -45., -12., -2., 0., 3., 12., 80., 800., 5000.];
    const COLORS: [[f64; 3]; 10] = [
        [36., 66., 130.],
        [50., 114., 170.],
        [72., 172., 190.],
        [126., 201., 190.],
        [175., 211., 174.],
        [210., 197., 137.],
        [212., 173., 110.],
        [180., 151., 114.],
        [176., 166., 161.],
        [240., 236., 228.],
    ];
    assert_eq!(values.len(), SIZE * SIZE);
    let mx = ((e - w) * 111320. * ((s + n) / 2.).to_radians().cos() / SIZE as f64).max(0.01);
    let my = ((n - s) * 111320. / SIZE as f64).max(0.01);
    let mut rgba = vec![0; SIZE * SIZE * 4];
    for (i, &height) in values.iter().enumerate() {
        if !height.is_finite() {
            continue;
        }
        let h = height as f64;
        let k = LEVELS.partition_point(|&v| v < h).clamp(1, 9);
        let f = ((h - LEVELS[k - 1]) / (LEVELS[k] - LEVELS[k - 1])).clamp(0., 1.);
        let gx = if i % SIZE > 0 && values[i - 1].is_finite() {
            (h - values[i - 1] as f64) / mx
        } else {
            0.
        };
        let gy = if i >= SIZE && values[i - SIZE].is_finite() {
            (h - values[i - SIZE] as f64) / my
        } else {
            0.
        };
        let light = (1. + 0.65 * (gy - gx) / (1. + gx * gx + gy * gy).sqrt()).clamp(0.45, 1.3);
        for band in 0..3 {
            rgba[4 * i + band] = ((COLORS[k - 1][band] * (1. - f) + COLORS[k][band] * f) * light)
                .clamp(0., 255.) as u8;
        }
        rgba[4 * i + 3] = 255;
    }
    rgba
}

/// None delegates an unprepared view to the existing source path. Prepared views
/// never launch a subprocess. Raw receipts guard against hiding pending newer scans.
pub fn render(
    catalog: &Path,
    bounds: [f64; 4],
    preferred: &str,
    active: Option<&Path>,
) -> Result<Option<Rendered>, String> {
    render_impl(catalog, bounds, preferred, active, true)
}

/// Prepared mosaic alone: pending raw scans do not hide completed neighbors.
pub fn render_prepared(
    catalog: &Path,
    bounds: [f64; 4],
    active: Option<&Path>,
) -> Result<Option<Rendered>, String> {
    render_impl(catalog, bounds, "", active, false)
}

fn render_impl(
    catalog: &Path,
    bounds: [f64; 4],
    preferred: &str,
    active: Option<&Path>,
    require_complete: bool,
) -> Result<Option<Rendered>, String> {
    let root = catalog.parent().ok_or("Missing lidar root")?;
    let [w, s, e, n] = bounds;
    let mut rows = Vec::new();
    for (index, table, url, rank) in [
        ("index.sqlite", "rasters", "url", "17"),
        ("previews.sqlite", "tiles", "key", "zoom"),
    ] {
        let index = root.join("fused/v1").join(index);
        if !index.is_file() {
            continue;
        }
        let db =
            Connection::open_with_flags(index, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(err)?;
        let mut query = db.prepare(&format!("SELECT {url},survey,path,date,{rank} FROM {table} WHERE west<?1 AND east>=?2
        AND south<?3 AND north>=?4 AND (?5='' OR survey=?5 OR (substr(?5,-1)='/' AND instr(survey,?5)=1))
        ORDER BY date DESC,survey,{url}")).map_err(err)?;
        let found = query
            .query_map(params![e, w, n, s, preferred], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, u8>(4)?,
                ))
            })
            .map_err(err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(err)?;
        rows.extend(found);
    }
    // Newest acquisition wins; its full-density raster precedes overviews.
    // Finer overviews precede coarser ones from the same survey/date.
    rows.sort_by(|a, b| {
        b.3.cmp(&a.3)
            .then(a.1.cmp(&b.1))
            .then(b.4.cmp(&a.4))
            .then(a.0.cmp(&b.0))
    });
    if rows.is_empty() {
        return Ok(None);
    }
    if require_complete
        && let Ok(bulk) =
            Connection::open_with_flags(root.join("bulk.sqlite"), OpenFlags::SQLITE_OPEN_READ_ONLY)
    {
        let mut stmt = bulk
            .prepare(
                "SELECT url FROM files WHERE west<?1 AND east>=?2 AND south<?3 AND north>=?4
            AND (?5='' OR survey=?5 OR (substr(?5,-1)='/' AND instr(survey,?5)=1))",
            )
            .map_err(err)?;
        let pending = stmt
            .query_map(params![e, w, n, s, preferred], |r| r.get::<_, String>(0))
            .map_err(err)?;
        for url in pending {
            let url = url.map_err(err)?;
            if !rows.iter().any(|r| r.0 == url) {
                return Ok(None);
            }
        }
    }
    let catalog =
        Connection::open_with_flags(catalog, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(err)?;
    let mut values = vec![f32::NAN; SIZE * SIZE];
    let mut credits = Vec::new();
    for (_, survey, path, _, _) in rows {
        if active.is_some_and(|p| !p.is_file()) {
            return Err("Camera moved".into());
        }
        let file = checked_path(root, &path)?;
        if !file.is_file() {
            continue;
        }
        let data = read_mean(&file, bounds)?;
        let mut used = false;
        for (dest, src) in values.iter_mut().zip(data) {
            if !dest.is_finite() && src.is_finite() {
                *dest = src;
                used = true;
            }
        }
        if used && !credits.iter().any(|c: &Credit| c[1] == survey) {
            let credit = catalog
                .query_row(
                    "SELECT name,id,page FROM surveys WHERE id=?1",
                    [&survey],
                    |r| Ok([r.get(0)?, r.get(1)?, r.get(2)?]),
                )
                .unwrap_or([survey.clone(), survey, String::new()]);
            credits.push(credit);
        }
        if values.iter().all(|v| v.is_finite()) {
            break;
        }
    }
    Ok(Some(Rendered {
        rgba: shade(&values, bounds),
        credits,
    }))
}

pub fn save_png(path: &Path, rendered: &Rendered) -> Result<(), String> {
    let parent = path.parent().ok_or("Missing output directory")?;
    std::fs::create_dir_all(parent).map_err(err)?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent).map_err(err)?;
    {
        let mut encoder = png::Encoder::new(tmp.as_file_mut(), SIZE as u32, SIZE as u32);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        // Match the existing ASCII JSON tEXt contract, including non-Latin
        // survey names and supplementary Unicode characters.
        let mut text = String::new();
        for ch in serde_json::to_string(&rendered.credits)
            .map_err(err)?
            .chars()
        {
            if ch.is_ascii() {
                text.push(ch);
            } else {
                for unit in ch.encode_utf16(&mut [0; 2]) {
                    use std::fmt::Write;
                    write!(&mut text, "\\u{unit:04x}").map_err(err)?;
                }
            }
        }
        encoder
            .add_text_chunk("Surveys".into(), text)
            .map_err(err)?;
        let mut writer = encoder.write_header().map_err(err)?;
        writer.write_image_data(&rendered.rgba).map_err(err)?;
        writer.finish().map_err(err)?;
    }
    tmp.persist(path).map_err(err)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unicode_credits_round_trip_through_the_existing_png_metadata_contract() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("map.png");
        let rendered = Rendered {
            rgba: vec![0; SIZE * SIZE * 4],
            credits: vec![[
                "測量 🌍".into(),
                "scan".into(),
                "https://example.org".into(),
            ]],
        };
        save_png(&path, &rendered).unwrap();
        let decoder = png::Decoder::new(BufReader::new(File::open(path).unwrap()));
        let mut reader = decoder.read_info().unwrap();
        let text = &reader.info().uncompressed_latin1_text[0].text;
        assert!(text.is_ascii());
        assert_eq!(
            serde_json::from_str::<Vec<Credit>>(text).unwrap(),
            rendered.credits
        );
        let mut buffer = vec![0; SIZE * SIZE * 4];
        reader.next_frame(&mut buffer).unwrap();
        assert_eq!(buffer, rendered.rgba);
    }
}
