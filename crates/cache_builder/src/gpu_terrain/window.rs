//! Decode just the TIFF chunks intersecting a source-pixel window.
use std::{
    fs::File,
    io::{BufReader, Read, Seek, SeekFrom},
    path::Path,
    time::Instant,
};
use tiff::decoder::{ChunkType, Decoder, DecodingResult};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Window {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}
impl Window {
    pub fn contains(self, other: Self) -> bool {
        self.x <= other.x
            && self.y <= other.y
            && self.x + self.width >= other.x + other.width
            && self.y + self.height >= other.y + other.height
    }
    /// Include the second bilinear neighbor, including at exact source posts.
    pub fn from_axes(axes: &[f32], n: usize) -> Option<Self> {
        let extent = |values: &[f32]| {
            let mut low = f32::INFINITY;
            let mut high = f32::NEG_INFINITY;
            for &v in values {
                if (0.0..=3600.0).contains(&v) {
                    low = low.min(v);
                    high = high.max(v);
                }
            }
            (low <= high).then(|| (low.floor() as u32, (high.floor() as u32 + 2).min(3601)))
        };
        let (x, end_x) = extent(axes.get(..n)?)?;
        let (y, end_y) = extent(axes.get(n..n.checked_mul(2)?)?)?;
        Some(Self {
            x,
            y,
            width: end_x - x,
            height: end_y - y,
        })
    }
}
struct CountedFile {
    file: File,
    bytes: u64,
    io_seconds: f64,
}
impl Read for CountedFile {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        let start = Instant::now();
        let result = self.file.read(b);
        self.io_seconds += start.elapsed().as_secs_f64();
        if let Ok(n) = &result {
            self.bytes += *n as u64;
        }
        result
    }
}
impl Seek for CountedFile {
    fn seek(&mut self, p: SeekFrom) -> std::io::Result<u64> {
        let start = Instant::now();
        let result = self.file.seek(p);
        self.io_seconds += start.elapsed().as_secs_f64();
        result
    }
}
pub(super) struct Decoded {
    pub window: Window,
    pub samples: Vec<f32>,
    pub chunks: u32,
    pub bytes_read: u64,
    pub io_ms: f64,
    pub cpu_ms: f64,
}
pub(super) fn load(path: &Path, needed: Window) -> Result<Decoded, String> {
    let start = Instant::now();
    let read = || -> Result<Decoded, Box<dyn std::error::Error>> {
        let mut decoder = Decoder::new(BufReader::new(CountedFile {
            file: File::open(path)?,
            bytes: 0,
            io_seconds: 0.0,
        }))?;
        if decoder.dimensions()? != (3601, 3601)
            || decoder.colortype()? != tiff::ColorType::Gray(16)
            || needed.width == 0
            || needed.height == 0
            || needed.x >= 3601
            || needed.y >= 3601
            || needed.width > 3601 - needed.x
            || needed.height > 3601 - needed.y
        {
            return Err("Unsupported SRTM window or pixel layout".into());
        }
        let total = match decoder.get_chunk_type() {
            ChunkType::Tile => decoder.tile_count()?,
            ChunkType::Strip => decoder.strip_count()?,
        };
        // Validate the fallible layout before using the dimension accessors.
        decoder.image_chunk_buffer_layout(0)?;
        let (cw, ch) = decoder.chunk_dimensions();
        if cw == 0 || ch == 0 {
            return Err("Invalid TIFF chunk dimensions".into());
        }
        let right = (u64::from(needed.x + needed.width).div_ceil(u64::from(cw)) * u64::from(cw))
            .min(3601) as u32;
        let bottom = (u64::from(needed.y + needed.height).div_ceil(u64::from(ch)) * u64::from(ch))
            .min(3601) as u32;
        let x = needed.x / cw * cw;
        let y = needed.y / ch * ch;
        let window = Window {
            x,
            y,
            width: right - x,
            height: bottom - y,
        };
        let mut samples = vec![super::NODATA; window.width as usize * window.height as usize];
        let columns = 3601u32.div_ceil(cw);
        let mut chunks = 0;
        for row in (y..bottom).step_by(ch as usize) {
            for col in (x..right).step_by(cw as usize) {
                let index = (row / ch) * columns + col / cw;
                if index >= total {
                    return Err("Invalid TIFF chunk index".into());
                }
                let DecodingResult::I16(data) = decoder.read_chunk(index)? else {
                    return Err("GPU SRTM windows require signed Int16 data".into());
                };
                let (width, height) = decoder.chunk_data_dimensions(index);
                if data.len() != width as usize * height as usize
                    || col + width > right
                    || row + height > bottom
                {
                    return Err("Invalid decoded TIFF chunk extent".into());
                }
                for dy in 0..height as usize {
                    let output = (row as usize - y as usize + dy) * window.width as usize
                        + (col - x) as usize;
                    for (dst, &v) in samples[output..output + width as usize]
                        .iter_mut()
                        .zip(&data[dy * width as usize..(dy + 1) * width as usize])
                    {
                        if v != i16::MIN && !(-12000..=10000).contains(&v) {
                            return Err("Unexpected SRTM elevation range".into());
                        }
                        *dst = f32::from(v);
                    }
                }
                chunks += 1;
            }
        }
        let reader = decoder.inner().get_ref();
        Ok(Decoded {
            window,
            samples,
            chunks,
            bytes_read: reader.bytes,
            io_ms: reader.io_seconds * 1000.0,
            cpu_ms: (start.elapsed().as_secs_f64() - reader.io_seconds) * 1000.0,
        })
    };
    read().map_err(|e| format!("SRTM window {}: {e}", path.display()))
}
