//! Binary cell format (`.1kc`) for 1kEE geographic feature cells.
//!
//! # Format Overview
//!
//! Every `.1kc` file begins with a 10-byte header followed by zero or more
//! typed chunks.  Chunks are self-describing: a 4-byte ASCII tag identifies
//! the payload type, a `u32` gives the payload byte length, and the payload
//! follows.  A reader that encounters an unknown tag skips `length` bytes and
//! continues — this provides forward compatibility as new chunk types are
//! added in future versions.
//!
//! ## File Header (10 bytes, all integers little-endian)
//! ```text
//! [0..4]  magic:    b"1kEE"
//! [4]     version:  u8 = 1
//! [5..7]  cell_lat: i16  (floor of the cell's min latitude)
//! [7..9]  cell_lon: i16  (floor of the cell's min longitude)
//! [9]     reserved: u8 = 0
//! ```
//!
//! ## Chunk Envelope (8 bytes + payload)
//! ```text
//! [0..4]  tag:    [u8; 4]  e.g. b"ROAD"
//! [4..8]  length: u32 LE   byte length of the payload that follows
//! [8..]   payload
//! ```
//!
//! ## Feature Payload (used by ROAD / WATR / BLDG / TREE / ADMN)
//! ```text
//! [0..4]  feature_count: u32 LE
//! per feature:
//!   way_id:       i64 LE
//!   class:        u8         (chunk-type-specific enum, see below)
//!   flags:        u8         bit 0 = is_polygon
//!                            bit 1 = has_name
//!                            bit 2 = has_elevation
//!   name_len:     u16 LE     0 when has_name = 0
//!   name:         [u8; name_len]  UTF-8, no null terminator
//!   point_count:  u32 LE
//!   points:       [(lon:f32 LE, lat:f32 LE)]            when has_elevation = 0  (8 B each)
//!              or [(lon:f32 LE, lat:f32 LE, elev:f32 LE)] when has_elevation = 1 (12 B each)
//! ```
//!
//! ## Class Enums
//!
//! | Chunk | 0        | 1      | 2       | 3         | 4        | 5     |
//! |-------|----------|--------|---------|-----------|----------|-------|
//! | ROAD  | motorway | trunk  | primary | secondary | tertiary | minor |
//! | WATR  | river    | stream |         |           |          |       |
//! | BLDG  | building |        |         |           |          |       |
//! | TREE  | forest   |        |         |           |          |       |
//! | ADMN  | class byte equals the admin_level value (2 / 4 / 6 / 8)   |
//!
//! ## Reserved Future Chunk Tags
//!
//! `BL3D` (3-D building geometry), `CNTR` (contour lines), `BATH`
//! (bathymetry isolines), `TRCK` (timestamped tracks), `HTRY`
//! (time-bounded historical features), `LABL` (point labels).

pub mod read;
pub mod write;

// ── File-level constants ────────────────────────────────────────────────────

pub const MAGIC: [u8; 4] = *b"1kEE";
pub const VERSION: u8 = 1;

// ── Chunk tags ──────────────────────────────────────────────────────────────

pub const TAG_ROAD: [u8; 4] = *b"ROAD";
pub const TAG_WATR: [u8; 4] = *b"WATR";
pub const TAG_BLDG: [u8; 4] = *b"BLDG";
pub const TAG_TREE: [u8; 4] = *b"TREE";
pub const TAG_ADMN: [u8; 4] = *b"ADMN";

// ── Feature flag bits ───────────────────────────────────────────────────────

pub const FLAG_IS_POLYGON: u8 = 0x01;
pub const FLAG_HAS_NAME: u8 = 0x02;
pub const FLAG_HAS_ELEVATION: u8 = 0x04;

// ── Data types ──────────────────────────────────────────────────────────────

/// A single coordinate.  Stored on disk as `(lon, lat)` — west-east first,
/// consistent with GeoJSON convention.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellPoint {
    pub lon: f32,
    pub lat: f32,
}

/// One geographic feature from a cell file.
///
/// `way_id` is the OSM way ID for vector features; for admin boundaries it
/// holds the relation ID.  `class` is a chunk-type-specific byte enum (see
/// module-level docs).  `elevations`, when `Some`, is parallel to `points`
/// and holds a pre-sampled elevation in metres per vertex — baked in at build
/// time so the desktop never needs to hit the SRTM tiles at load time.
#[derive(Clone, Debug)]
pub struct CellFeature {
    pub way_id: i64,
    pub class: u8,
    pub is_polygon: bool,
    pub name: Option<String>,
    pub points: Vec<CellPoint>,
    /// Pre-sampled elevation (metres) per vertex.  `None` until the builder
    /// is taught to sample SRTM; the desktop falls back to runtime SRTM
    /// sampling when this is absent.
    pub elevations: Option<Vec<f32>>,
}

// ── Path helpers ─────────────────────────────────────────────────────────────

/// Return the filename (no directory) for a geographic cell.
///
/// Matches the existing directory conventions:
/// `{prefix}_cell_{:+04}_{:+05}.1kc`
pub fn cell_filename(prefix: &str, cell_lat: i32, cell_lon: i32) -> String {
    format!("{prefix}_cell_{cell_lat:+04}_{cell_lon:+05}.1kc")
}

/// Return the filename for a per-level admin boundary file.
///
/// `admin_level_{level}.1kc`
pub fn admin_filename(admin_level: u8) -> String {
    format!("admin_level_{admin_level}.1kc")
}

// ── Class encode / decode ────────────────────────────────────────────────────

pub fn encode_road_class(class: &str) -> u8 {
    match class {
        "motorway" => 0,
        "trunk" => 1,
        "primary" => 2,
        "secondary" => 3,
        "tertiary" => 4,
        _ => 5,
    }
}

pub fn decode_road_class(byte: u8) -> &'static str {
    match byte {
        0 => "motorway",
        1 => "trunk",
        2 => "primary",
        3 => "secondary",
        4 => "tertiary",
        _ => "minor",
    }
}

pub fn encode_watr_class(class: &str) -> u8 {
    match class {
        "river" => 0,
        _ => 1,
    }
}

pub fn decode_watr_class(byte: u8) -> &'static str {
    match byte {
        0 => "river",
        _ => "stream",
    }
}

/// Decode a class byte to its canonical string given the chunk tag.
pub fn decode_class(tag: &[u8; 4], byte: u8) -> &'static str {
    match tag {
        b"ROAD" => decode_road_class(byte),
        b"WATR" => decode_watr_class(byte),
        b"BLDG" => "building",
        b"TREE" => "forest",
        _ => "unknown",
    }
}
