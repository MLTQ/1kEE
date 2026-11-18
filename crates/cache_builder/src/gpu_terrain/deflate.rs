//! Read native 512² Deflate tiles in file order and inflate while the drive reads ahead.
use super::window::{TiffReader, Window};
use libdeflater::Decompressor;
use std::{
    collections::VecDeque,
    io::{self, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::mpsc,
    time::{Duration, Instant, SystemTime},
};
use tiff::{
    decoder::ChunkType,
    tags::{ByteOrder, Tag},
};

const SIDE: u32 = 512;
const COLUMNS: u32 = 3601u32.div_ceil(SIDE);
const TILE_BYTES: usize = (SIDE * SIDE * 2) as usize;
// Retain only blocks that straddle the requested window, up to 32 MiB.
const CACHE_BLOCKS: usize = 32 * 1024 * 1024 / TILE_BYTES;

struct Entry {
    path: PathBuf,
    modified: SystemTime,
    size: u64,
    index: u32,
    pixels: Vec<i16>,
}

#[derive(Default)]
pub(super) struct BlockCache(VecDeque<Entry>);

#[derive(Clone, Copy)]
struct Block {
    index: u32,
    offset: u64,
    end: u64,
}

struct ReadBatch {
    blocks: Vec<Block>,
    bytes: Vec<u8>,
}

fn read_batches(
    reader: &mut (impl Read + Seek),
    blocks: &mut [Block],
    mut consume: impl FnMut(ReadBatch) -> io::Result<()>,
) -> io::Result<()> {
    blocks.sort_unstable_by_key(|b| b.offset);
    let mut rest: &[Block] = blocks;
    while let Some(first) = rest.first() {
        let mut count = 1;
        // Keep reads bounded; skip unrelated tiles and overviews in sparse windows.
        while count < rest.len().min(4)
            && rest[count].offset >= rest[count - 1].end
            && rest[count].offset - rest[count - 1].end <= 4096
        {
            count += 1;
        }
        let (run, tail) = rest.split_at(count);
        let length = (run[count - 1].end - first.offset) as usize;
        let mut bytes = vec![0; length];
        reader.seek(SeekFrom::Start(first.offset))?;
        reader.read_exact(&mut bytes)?;
        consume(ReadBatch {
            blocks: run.to_vec(),
            bytes,
        })?;
        rest = tail;
    }
    Ok(())
}

// The caller decodes directly into the requested window while one reader
// fetches the next compressed batch. Neither side accumulates a whole file.
fn read_ahead(
    reader: &mut (impl Read + Seek + Send),
    blocks: &mut [Block],
    mut decode: impl FnMut(ReadBatch) -> io::Result<()>,
) -> io::Result<()> {
    if blocks.len() < 3 {
        return read_batches(reader, blocks, decode);
    }
    std::thread::scope(|scope| {
        let (send, receive) = mpsc::sync_channel(1);
        let io = std::thread::Builder::new()
            .name("srtm-block-read".into())
            .spawn_scoped(scope, move || {
                read_batches(reader, blocks, |batch| {
                    send.send(batch)
                        .map_err(|_| io::Error::other("TIFF decoder stopped"))
                })
            })?;
        // Dropping the receiver on a decode error wakes a blocked reader.
        let decoded = receive.into_iter().try_for_each(&mut decode);
        let read = io
            .join()
            .map_err(|_| io::Error::other("TIFF reader panicked"))?;
        decoded.and(read)
    })
}

fn copy_block(
    index: u32,
    pixels: &[i16],
    window: Window,
    samples: &mut [i16],
) -> io::Result<Window> {
    let x = index % COLUMNS * SIDE;
    let y = index / COLUMNS * SIDE;
    let width = SIDE.min(3601 - x);
    let height = SIDE.min(3601 - y);
    for dy in 0..height as usize {
        let row = dy * SIDE as usize;
        let start =
            (y as usize - window.y as usize + dy) * window.width as usize + (x - window.x) as usize;
        super::window::copy_heights(
            &mut samples[start..start + width as usize],
            &pixels[row..row + width as usize],
        )
        .map_err(io::Error::other)?;
    }
    Ok(Window {
        x,
        y,
        width,
        height,
    })
}

pub(super) struct Stats {
    pub hits: u32,
    pub cpu: Duration,
}

pub(super) fn read_window(
    decoder: &mut TiffReader,
    path: &Path,
    cache: &mut BlockCache,
    needed: Window,
    window: Window,
    samples: &mut [i16],
) -> Result<Option<Stats>, Box<dyn std::error::Error>> {
    if decoder.get_chunk_type() != ChunkType::Tile
        || decoder.chunk_dimensions() != (SIDE, SIDE)
        || decoder.tile_count()? != COLUMNS * COLUMNS
        || !matches!(decoder.get_tag_u32(Tag::Compression)?, 8 | 32946)
        || decoder
            .find_tag_unsigned::<u32>(Tag::Predictor)?
            .unwrap_or(1)
            != 1
        || decoder.get_tag_u32(Tag::PhotometricInterpretation)? != 1
        || decoder.get_tag_u16_vec(Tag::SampleFormat)? != [2]
    {
        return Ok(None);
    }
    let offsets = decoder.get_tag_u64_vec(Tag::TileOffsets)?;
    let lengths = decoder.get_tag_u64_vec(Tag::TileByteCounts)?;
    let order = decoder.byte_order();
    let reader = decoder.inner();
    let meta = reader.get_ref().file.metadata()?;
    let modified = meta.modified().ok();
    let mut hits = 0;
    let mut cpu = Duration::ZERO;
    let mut missing = Vec::new();
    for y in (window.y..window.y + window.height).step_by(SIDE as usize) {
        for x in (window.x..window.x + window.width).step_by(SIDE as usize) {
            let index = y / SIDE * COLUMNS + x / SIDE;
            if let Some(pos) = cache.0.iter().position(|e| {
                e.index == index
                    && Some(e.modified) == modified
                    && e.size == meta.len()
                    && e.path == path
            }) {
                let entry = cache.0.remove(pos).unwrap();
                let start = Instant::now();
                copy_block(index, &entry.pixels, window, samples)?;
                cpu += start.elapsed();
                hits += 1;
                cache.0.push_back(entry);
                continue;
            }
            let offset = *offsets
                .get(index as usize)
                .ok_or("missing TIFF block offset")?;
            let length = *lengths
                .get(index as usize)
                .ok_or("missing TIFF block length")?;
            let end = offset
                .checked_add(length)
                .ok_or("invalid TIFF block range")?;
            // Other legal TIFF encodings and sparse blocks use the general decoder.
            if length == 0 || length > 2 * TILE_BYTES as u64 {
                return Ok(None);
            }
            if end > meta.len() {
                return Err("truncated TIFF block".into());
            }
            missing.push(Block { index, offset, end });
        }
    }
    if missing.is_empty() {
        return Ok(Some(Stats { hits, cpu }));
    }
    let mut inflate = Decompressor::new();
    let mut pixels = vec![0i16; TILE_BYTES / 2];
    read_ahead(reader, &mut missing, |batch| {
        let start = Instant::now();
        let base = batch.blocks[0].offset;
        for block in batch.blocks {
            let bytes = &batch.bytes[(block.offset - base) as usize..(block.end - base) as usize];
            let written = inflate
                .zlib_decompress(bytes, bytemuck::cast_slice_mut(&mut pixels))
                .map_err(io::Error::other)?;
            if written != TILE_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "incomplete TIFF Deflate block",
                ));
            }
            if order != ByteOrder::native() {
                pixels.iter_mut().for_each(|p| *p = p.swap_bytes());
            }
            let bounds = copy_block(block.index, &pixels, window, samples)?;
            if !needed.contains(bounds)
                && let Some(modified) = modified
            {
                // Recycle the evicted block as the next decoder scratch area.
                let next = if cache.0.len() == CACHE_BLOCKS {
                    cache.0.pop_front().unwrap().pixels
                } else {
                    vec![0i16; TILE_BYTES / 2]
                };
                cache.0.push_back(Entry {
                    path: path.into(),
                    modified,
                    size: meta.len(),
                    index: block.index,
                    pixels: std::mem::replace(&mut pixels, next),
                });
            }
        }
        cpu += start.elapsed();
        Ok(())
    })?;
    Ok(Some(Stats { hits, cpu }))
}
