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
//! ## Feature Payload (used by all chunk types)
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
//! | Chunk | 0              | 1              | 2            | 3            | 4            | 5             | 6           | 7           | 8        |
//! |-------|----------------|----------------|--------------|--------------|--------------|---------------|-------------|-------------|----------|
//! | ROAD  | motorway       | trunk          | primary      | secondary    | tertiary     | minor         |             |             |          |
//! | WATR  | river          | stream         |              |              |              |               |             |             |          |
//! | BLDG  | building       |                |              |              |              |               |             |             |          |
//! | TREE  | forest         |                |              |              |              |               |             |             |          |
//! | ADMN  | class byte equals the admin_level value (2 / 4 / 6 / 8)                                                                          |
//! | POWR  | line ≥300kV    | line 100-299kV | line 50-99kV | line <50kV   | minor_line   | substation    | power_plant | tower/pylon |          |
//! | RAIL  | mainline       | rail           | subway/metro | tram         | light_rail   | narrow_gauge  | funicular   | monorail    | disused  |
//! | PIPE  | gas            | oil            | water        | sewer        | other        |               |             |             |          |
//! | AERO  | intl airport   | dom airport    | airfield     | helipad      | airstrip     | terminal      | runway      |             |          |
//! | MILT  | base/install   | danger area    | airbase      | naval base   | barracks     | checkpoint    |             |             |          |
//! | COMM  | comm tower     | antenna/mast   | radar        | telephone_ex | data_center  | satellite     |             |             |          |
//! | INDS  | industrial     | factory/works  | power_plant  | mine/quarry  | oil_terminal | refinery      | storage     |             |          |
//! | PORT  | harbour        | ferry_terminal | marina       | shipyard     | lighthouse   | buoy/marker   | ship_lane   |             |          |
//! | GOVT  | border_cross   | embassy        | customs      | police       | fire_station | prison        | courthouse  | govt_bldg  |          |
//! | SURV  | cctv           | speed_camera   | surv_station | police_check | border_post  |               |             |             |          |
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
pub const TAG_POWR: [u8; 4] = *b"POWR"; // power lines, substations, power plants
pub const TAG_RAIL: [u8; 4] = *b"RAIL"; // railways, metros, trams
pub const TAG_PIPE: [u8; 4] = *b"PIPE"; // gas, oil, water, sewer pipelines
pub const TAG_AERO: [u8; 4] = *b"AERO"; // airports, helipads, runways
pub const TAG_MILT: [u8; 4] = *b"MILT"; // military bases, danger areas
pub const TAG_COMM: [u8; 4] = *b"COMM"; // communication towers, radar, data centres
pub const TAG_INDS: [u8; 4] = *b"INDS"; // industrial areas, mines, refineries
pub const TAG_PORT: [u8; 4] = *b"PORT"; // harbours, marinas, lighthouses
pub const TAG_GOVT: [u8; 4] = *b"GOVT"; // government buildings, embassies, prisons
pub const TAG_SURV: [u8; 4] = *b"SURV"; // surveillance cameras, radar, checkpoints

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
        b"POWR" => decode_powr_class(byte),
        b"RAIL" => decode_rail_class(byte),
        b"PIPE" => decode_pipe_class(byte),
        b"AERO" => decode_aero_class(byte),
        b"MILT" => decode_milt_class(byte),
        b"COMM" => decode_comm_class(byte),
        b"INDS" => decode_inds_class(byte),
        b"PORT" => decode_port_class(byte),
        b"GOVT" => decode_govt_class(byte),
        b"SURV" => decode_surv_class(byte),
        _ => "unknown",
    }
}

// ── POWR class byte constants ────────────────────────────────────────────────

/// Transmission line, voltage ≥ 300 kV — highest LOD priority.
pub const POWR_LINE_ULTRA: u8 = 0;
/// Transmission line, voltage 100–299 kV.
pub const POWR_LINE_HIGH: u8 = 1;
/// Transmission line, voltage 50–99 kV.
pub const POWR_LINE_MED: u8 = 2;
/// Distribution line, voltage < 50 kV.
pub const POWR_LINE_LOW: u8 = 3;
/// Minor/service line (typically untagged voltage).
pub const POWR_LINE_MINOR: u8 = 4;
/// Electrical substation (polygon or node).
pub const POWR_SUBSTATION: u8 = 5;
/// Power generation plant (polygon).
pub const POWR_PLANT: u8 = 6;
/// Individual tower/pylon (node → single-point feature).
pub const POWR_TOWER: u8 = 7;

pub fn encode_powr_class(power_type: &str, voltage_kv: Option<i32>) -> u8 {
    match power_type {
        "line" | "cable" => match voltage_kv {
            Some(v) if v >= 300 => POWR_LINE_ULTRA,
            Some(v) if v >= 100 => POWR_LINE_HIGH,
            Some(v) if v >= 50 => POWR_LINE_MED,
            Some(v) if v > 0 => POWR_LINE_LOW,
            _ => POWR_LINE_MINOR,
        },
        "minor_line" => POWR_LINE_MINOR,
        "substation" | "sub_station" => POWR_SUBSTATION,
        "plant" | "generator" => POWR_PLANT,
        "tower" | "pole" => POWR_TOWER,
        _ => POWR_LINE_MINOR,
    }
}

pub fn decode_powr_class(byte: u8) -> &'static str {
    match byte {
        POWR_LINE_ULTRA => "line_ultra",
        POWR_LINE_HIGH => "line_high",
        POWR_LINE_MED => "line_med",
        POWR_LINE_LOW => "line_low",
        POWR_LINE_MINOR => "minor_line",
        POWR_SUBSTATION => "substation",
        POWR_PLANT => "power_plant",
        POWR_TOWER => "tower",
        _ => "unknown",
    }
}

/// Encode from the canonical class name string (as returned by `decode_powr_class`).
pub fn encode_powr_class_from_name(name: &str) -> u8 {
    match name {
        "line_ultra" => POWR_LINE_ULTRA,
        "line_high" => POWR_LINE_HIGH,
        "line_med" => POWR_LINE_MED,
        "line_low" => POWR_LINE_LOW,
        "minor_line" => POWR_LINE_MINOR,
        "substation" => POWR_SUBSTATION,
        "power_plant" => POWR_PLANT,
        "tower" => POWR_TOWER,
        _ => POWR_LINE_MINOR,
    }
}

// ── RAIL class byte constants ────────────────────────────────────────────────

pub const RAIL_MAINLINE: u8 = 0;
pub const RAIL_RAIL: u8 = 1;
pub const RAIL_SUBWAY: u8 = 2;
pub const RAIL_TRAM: u8 = 3;
pub const RAIL_LIGHT_RAIL: u8 = 4;
pub const RAIL_NARROW_GAUGE: u8 = 5;
pub const RAIL_FUNICULAR: u8 = 6;
pub const RAIL_MONORAIL: u8 = 7;
pub const RAIL_DISUSED: u8 = 8;

pub fn encode_rail_class(railway: &str) -> u8 {
    match railway {
        "mainline" => RAIL_MAINLINE,
        "rail" => RAIL_RAIL,
        "subway" => RAIL_SUBWAY,
        "tram" => RAIL_TRAM,
        "light_rail" => RAIL_LIGHT_RAIL,
        "narrow_gauge" => RAIL_NARROW_GAUGE,
        "funicular" | "cable_car" => RAIL_FUNICULAR,
        "monorail" => RAIL_MONORAIL,
        "disused" | "abandoned" | "razed" => RAIL_DISUSED,
        _ => RAIL_RAIL,
    }
}

pub fn decode_rail_class(byte: u8) -> &'static str {
    match byte {
        RAIL_MAINLINE => "mainline",
        RAIL_RAIL => "rail",
        RAIL_SUBWAY => "subway",
        RAIL_TRAM => "tram",
        RAIL_LIGHT_RAIL => "light_rail",
        RAIL_NARROW_GAUGE => "narrow_gauge",
        RAIL_FUNICULAR => "funicular",
        RAIL_MONORAIL => "monorail",
        RAIL_DISUSED => "disused",
        _ => "rail",
    }
}

// ── PIPE class byte constants ────────────────────────────────────────────────

pub const PIPE_GAS: u8 = 0;
pub const PIPE_OIL: u8 = 1;
pub const PIPE_WATER: u8 = 2;
pub const PIPE_SEWER: u8 = 3;
pub const PIPE_OTHER: u8 = 4;

pub fn encode_pipe_class(substance: &str) -> u8 {
    match substance {
        "gas" | "natural_gas" | "lpg" => PIPE_GAS,
        "oil" | "fuel" | "petroleum" | "kerosene" | "diesel" => PIPE_OIL,
        "water" | "rainwater" | "drinking_water" => PIPE_WATER,
        "sewage" | "wastewater" | "sewer" => PIPE_SEWER,
        _ => PIPE_OTHER,
    }
}

pub fn decode_pipe_class(byte: u8) -> &'static str {
    match byte {
        PIPE_GAS => "gas",
        PIPE_OIL => "oil",
        PIPE_WATER => "water",
        PIPE_SEWER => "sewer",
        _ => "other",
    }
}

// ── AERO class byte constants ────────────────────────────────────────────────

pub const AERO_INTL_AIRPORT: u8 = 0;
pub const AERO_DOM_AIRPORT: u8 = 1;
pub const AERO_AIRFIELD: u8 = 2;
pub const AERO_HELIPAD: u8 = 3;
pub const AERO_AIRSTRIP: u8 = 4;
pub const AERO_TERMINAL: u8 = 5;
pub const AERO_RUNWAY: u8 = 6;

pub fn decode_aero_class(byte: u8) -> &'static str {
    match byte {
        AERO_INTL_AIRPORT => "intl_airport",
        AERO_DOM_AIRPORT => "dom_airport",
        AERO_AIRFIELD => "airfield",
        AERO_HELIPAD => "helipad",
        AERO_AIRSTRIP => "airstrip",
        AERO_TERMINAL => "terminal",
        AERO_RUNWAY => "runway",
        _ => "airfield",
    }
}

pub fn encode_aero_class(name: &str) -> u8 {
    match name {
        "intl_airport" => AERO_INTL_AIRPORT,
        "dom_airport" => AERO_DOM_AIRPORT,
        "helipad" => AERO_HELIPAD,
        "airstrip" => AERO_AIRSTRIP,
        "terminal" => AERO_TERMINAL,
        "runway" => AERO_RUNWAY,
        _ => AERO_AIRFIELD,
    }
}

// ── MILT class byte constants ────────────────────────────────────────────────

pub const MILT_BASE: u8 = 0;
pub const MILT_DANGER: u8 = 1;
pub const MILT_AIRBASE: u8 = 2;
pub const MILT_NAVAL: u8 = 3;
pub const MILT_BARRACKS: u8 = 4;
pub const MILT_CHECKPOINT: u8 = 5;

pub fn decode_milt_class(byte: u8) -> &'static str {
    match byte {
        MILT_BASE => "base",
        MILT_DANGER => "danger_area",
        MILT_AIRBASE => "airbase",
        MILT_NAVAL => "naval_base",
        MILT_BARRACKS => "barracks",
        MILT_CHECKPOINT => "checkpoint",
        _ => "base",
    }
}

pub fn encode_milt_class(name: &str) -> u8 {
    match name {
        "danger_area" => MILT_DANGER,
        "airbase" => MILT_AIRBASE,
        "naval_base" => MILT_NAVAL,
        "barracks" => MILT_BARRACKS,
        "checkpoint" => MILT_CHECKPOINT,
        _ => MILT_BASE,
    }
}

// ── COMM class byte constants ────────────────────────────────────────────────

pub const COMM_TOWER: u8 = 0;
pub const COMM_ANTENNA: u8 = 1;
pub const COMM_RADAR: u8 = 2;
pub const COMM_TELEPHONE_EX: u8 = 3;
pub const COMM_DATA_CENTER: u8 = 4;
pub const COMM_SATELLITE: u8 = 5;

pub fn decode_comm_class(byte: u8) -> &'static str {
    match byte {
        COMM_TOWER => "comm_tower",
        COMM_ANTENNA => "antenna",
        COMM_RADAR => "radar",
        COMM_TELEPHONE_EX => "telephone_exchange",
        COMM_DATA_CENTER => "data_center",
        COMM_SATELLITE => "satellite_dish",
        _ => "comm_tower",
    }
}

pub fn encode_comm_class(name: &str) -> u8 {
    match name {
        "radar" => COMM_RADAR,
        "telephone_exchange" => COMM_TELEPHONE_EX,
        "data_center" => COMM_DATA_CENTER,
        "satellite_dish" => COMM_SATELLITE,
        "antenna" => COMM_ANTENNA,
        _ => COMM_TOWER,
    }
}

// ── INDS class byte constants ────────────────────────────────────────────────

pub const INDS_AREA: u8 = 0;
pub const INDS_FACTORY: u8 = 1;
pub const INDS_POWER_PLANT: u8 = 2;
pub const INDS_MINE: u8 = 3;
pub const INDS_OIL_TERMINAL: u8 = 4;
pub const INDS_REFINERY: u8 = 5;
pub const INDS_STORAGE: u8 = 6;

pub fn decode_inds_class(byte: u8) -> &'static str {
    match byte {
        INDS_AREA => "industrial",
        INDS_FACTORY => "factory",
        INDS_POWER_PLANT => "power_plant",
        INDS_MINE => "mine",
        INDS_OIL_TERMINAL => "oil_terminal",
        INDS_REFINERY => "refinery",
        INDS_STORAGE => "storage",
        _ => "industrial",
    }
}

pub fn encode_inds_class(name: &str) -> u8 {
    match name {
        "factory" => INDS_FACTORY,
        "power_plant" => INDS_POWER_PLANT,
        "mine" => INDS_MINE,
        "oil_terminal" => INDS_OIL_TERMINAL,
        "refinery" => INDS_REFINERY,
        "storage" => INDS_STORAGE,
        _ => INDS_AREA,
    }
}

// ── PORT class byte constants ────────────────────────────────────────────────

pub const PORT_HARBOUR: u8 = 0;
pub const PORT_FERRY_TERMINAL: u8 = 1;
pub const PORT_MARINA: u8 = 2;
pub const PORT_SHIPYARD: u8 = 3;
pub const PORT_LIGHTHOUSE: u8 = 4;
pub const PORT_BUOY: u8 = 5;
pub const PORT_SHIP_LANE: u8 = 6;

pub fn decode_port_class(byte: u8) -> &'static str {
    match byte {
        PORT_HARBOUR => "harbour",
        PORT_FERRY_TERMINAL => "ferry_terminal",
        PORT_MARINA => "marina",
        PORT_SHIPYARD => "shipyard",
        PORT_LIGHTHOUSE => "lighthouse",
        PORT_BUOY => "buoy",
        PORT_SHIP_LANE => "ship_lane",
        _ => "harbour",
    }
}

pub fn encode_port_class(name: &str) -> u8 {
    match name {
        "ferry_terminal" => PORT_FERRY_TERMINAL,
        "marina" => PORT_MARINA,
        "shipyard" => PORT_SHIPYARD,
        "lighthouse" => PORT_LIGHTHOUSE,
        "buoy" => PORT_BUOY,
        "ship_lane" => PORT_SHIP_LANE,
        _ => PORT_HARBOUR,
    }
}

// ── GOVT class byte constants ────────────────────────────────────────────────

pub const GOVT_BORDER_CROSSING: u8 = 0;
pub const GOVT_EMBASSY: u8 = 1;
pub const GOVT_CUSTOMS: u8 = 2;
pub const GOVT_POLICE: u8 = 3;
pub const GOVT_FIRE_STATION: u8 = 4;
pub const GOVT_PRISON: u8 = 5;
pub const GOVT_COURTHOUSE: u8 = 6;
pub const GOVT_BUILDING: u8 = 7;

pub fn decode_govt_class(byte: u8) -> &'static str {
    match byte {
        GOVT_BORDER_CROSSING => "border_crossing",
        GOVT_EMBASSY => "embassy",
        GOVT_CUSTOMS => "customs",
        GOVT_POLICE => "police",
        GOVT_FIRE_STATION => "fire_station",
        GOVT_PRISON => "prison",
        GOVT_COURTHOUSE => "courthouse",
        GOVT_BUILDING => "government",
        _ => "government",
    }
}

pub fn encode_govt_class(name: &str) -> u8 {
    match name {
        "border_crossing" => GOVT_BORDER_CROSSING,
        "embassy" => GOVT_EMBASSY,
        "customs" => GOVT_CUSTOMS,
        "police" => GOVT_POLICE,
        "fire_station" => GOVT_FIRE_STATION,
        "prison" => GOVT_PRISON,
        "courthouse" => GOVT_COURTHOUSE,
        _ => GOVT_BUILDING,
    }
}

// ── SURV class byte constants ────────────────────────────────────────────────

pub const SURV_CCTV: u8 = 0;
pub const SURV_SPEED_CAMERA: u8 = 1;
pub const SURV_STATION: u8 = 2;
pub const SURV_POLICE_CHECK: u8 = 3;
pub const SURV_BORDER_POST: u8 = 4;

pub fn decode_surv_class(byte: u8) -> &'static str {
    match byte {
        SURV_CCTV => "cctv",
        SURV_SPEED_CAMERA => "speed_camera",
        SURV_STATION => "surveillance_station",
        SURV_POLICE_CHECK => "police_checkpoint",
        SURV_BORDER_POST => "border_post",
        _ => "cctv",
    }
}

pub fn encode_surv_class(name: &str) -> u8 {
    match name {
        "speed_camera" => SURV_SPEED_CAMERA,
        "surveillance_station" => SURV_STATION,
        "police_checkpoint" => SURV_POLICE_CHECK,
        "border_post" => SURV_BORDER_POST,
        _ => SURV_CCTV,
    }
}
