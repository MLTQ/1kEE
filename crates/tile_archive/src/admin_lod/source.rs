use super::Result;
use cell_format::{CellPoint, FLAG_HAS_ELEVATION, FLAG_IS_POLYGON, TAG_ADMN};
use std::fs::File;
use std::io::{BufReader, Read, Seek};

pub struct Ring {
    pub relation: i64,
    pub polygon: bool,
    pub level: u8,
    pub offset: u64,
    pub stride: usize,
    pub points: Vec<CellPoint>,
}

pub fn decode(bytes: &[u8], stride: usize) -> Vec<CellPoint> {
    bytes
        .chunks_exact(stride)
        .map(|b| CellPoint {
            lon: f32::from_le_bytes(b[..4].try_into().unwrap()),
            lat: f32::from_le_bytes(b[4..8].try_into().unwrap()),
        })
        .collect()
}

/// Visit a v1 ADMN file ring by ring; a hostile point count cannot allocate past
/// the declared payload. Offsets are retained for direct reads of original detail.
pub fn visit(file: &mut File, mut ring: impl FnMut(Ring) -> Result<()>) -> Result<()> {
    let size = file.metadata()?.len();
    let mut input = BufReader::new(file);
    let mut head = [0; 10];
    input.read_exact(&mut head)?;
    cell_format::read::check_header(&head).ok_or("Invalid boundary cell header")?;
    loop {
        let mut envelope = [0; 8];
        input.read_exact(&mut envelope)?;
        let length = u32::from_le_bytes(envelope[4..].try_into().unwrap()) as u64;
        let mut pos = input.stream_position()?;
        let end = pos + length;
        if end > size {
            return Err("Truncated boundary chunk".into());
        }
        if envelope[..4] != TAG_ADMN {
            input.seek(std::io::SeekFrom::Start(end))?;
            continue;
        }
        let mut count = [0; 4];
        input.read_exact(&mut count)?;
        pos += 4;
        for _ in 0..u32::from_le_bytes(count) {
            let mut tag = [0; 12];
            input.read_exact(&mut tag)?;
            let relation = i64::from_le_bytes(tag[..8].try_into().unwrap());
            let n = u16::from_le_bytes(tag[10..].try_into().unwrap());
            input.seek_relative(n.into())?;
            input.read_exact(&mut count)?;
            pos += 16 + u64::from(n);
            let stride = if tag[9] & FLAG_HAS_ELEVATION == 0 {
                8
            } else {
                12
            };
            let bytes = u64::from(u32::from_le_bytes(count)) * stride as u64;
            if pos + bytes > end {
                return Err("Invalid boundary coordinate count".into());
            }
            let mut buf = vec![0; bytes.try_into()?];
            input.read_exact(&mut buf)?;
            ring(Ring {
                relation,
                level: tag[8],
                polygon: tag[9] & FLAG_IS_POLYGON != 0,
                offset: pos,
                stride,
                points: decode(&buf, stride),
            })?;
            pos += bytes;
        }
        if pos != end {
            return Err("Invalid boundary payload length".into());
        }
        return Ok(());
    }
}
