pub mod db;
pub mod inventory;
pub mod job_dispatch;
pub mod roads_global;
pub mod roads_osmium;
pub mod roads_overpass;
pub mod roads_stream;
pub mod util;
pub mod water;

use crate::model::GeoPoint;
use std::path::{Path, PathBuf};
use std::thread::JoinHandle;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

pub(crate) const PLANET_PBF_NAME: &str = "planet-latest.osm.pbf";
pub(crate) const RUNTIME_DB_NAME: &str = "osm_runtime.sqlite";
pub(crate) const PLANET_ROADS_NOTE: &str = "planet_roads_bootstrap_v1";
pub(crate) const FOCUS_ROADS_NOTE_PREFIX: &str = "focus_roads_v1";
pub(crate) const FOCUS_WATER_NOTE_PREFIX: &str = "focus_water";
pub(crate) const ROAD_TILE_ZOOMS: &[u8] = &[4, 6, 8, 10];
pub(crate) const PROGRESS_FLUSH_INTERVAL: usize = 25_000;
pub(crate) const FOCUS_SCAN_PROGRESS_INTERVAL: usize = 2_000_000;
pub(crate) const FOCUS_NODE_MARGIN_DEGREES: f32 = 0.08;
#[allow(dead_code)]
pub(crate) const DEFAULT_FOCUS_RADIUS_MILES: f32 = 20.0;
/// Sentinel source path used when queuing an Overpass-backed focus job.
pub(crate) const OVERPASS_SOURCE: &str = "overpass";
pub(crate) const OVERPASS_ENDPOINT: &str = "https://overpass-api.de/api/interpreter";

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OsmFeatureKind {
    Roads,
    Buildings,
    Water,
}

impl OsmFeatureKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Roads => "roads",
            Self::Buildings => "buildings",
            Self::Water => "water",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct GeoBounds {
    pub min_lat: f32,
    pub max_lat: f32,
    pub min_lon: f32,
    pub max_lon: f32,
}

pub struct OsmInventory {
    pub planet_path: Option<PathBuf>,
    pub planet_size_bytes: u64,
    pub runtime_db_path: Option<PathBuf>,
    pub runtime_db_ready: bool,
    pub queued_jobs: usize,
    pub road_tiles: usize,
    pub building_tiles: usize,
    #[allow(dead_code)]
    pub water_tiles: usize,
    pub primary_runtime_source: &'static str,
}

/// A water feature polyline from OSM.
#[derive(Clone, Debug)]
pub struct WaterPolyline {
    #[allow(dead_code)]
    pub way_id: i64,
    #[allow(dead_code)]
    pub water_class: String,
    #[allow(dead_code)]
    pub name: Option<String>,
    pub points: Vec<GeoPoint>,
    pub is_area: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoadLayerKind {
    Major,
    Minor,
}

#[derive(Clone, Debug)]
pub struct RoadPolyline {
    #[allow(dead_code)]
    pub way_id: i64,
    #[allow(dead_code)]
    pub road_class: String,
    #[allow(dead_code)]
    pub name: Option<String>,
    pub points: Vec<GeoPoint>,
}

#[derive(Clone)]
pub struct OsmJobSnapshot {
    pub label: String,
    pub state: String,
    pub note: String,
}

#[derive(Clone)]
pub(crate) struct OsmJob {
    pub(crate) id: i64,
    pub(crate) feature_kind: OsmFeatureKind,
    pub(crate) source_path: PathBuf,
    pub(crate) bounds: GeoBounds,
    pub(crate) note: String,
}

pub(crate) struct ActiveWorker {
    pub(crate) handle: JoinHandle<()>,
}

// ---------------------------------------------------------------------------
// Public API — re-exported from sub-modules
// ---------------------------------------------------------------------------

pub use inventory::find_planet_pbf;
pub use inventory::supports_locations_on_ways;
#[allow(dead_code)]
pub use inventory::validate_reader;

pub use job_dispatch::{
    active_job_note, ensure_runtime_store, has_active_jobs, osmium_cell_progress,
    queue_focus_roads_import, queue_focus_water_import, queue_planet_roads_import,
    queue_region_job, road_data_generation, snapshots, tick, water_data_generation,
};

pub use util::lat_lon_to_tile;

pub use water::load_water_for_bounds;

/// Load road polylines for the given bounds from the runtime SQLite DB.
pub fn load_roads_for_bounds(
    selected_root: Option<&Path>,
    bounds: GeoBounds,
    tile_zoom: u8,
    layer_kind: RoadLayerKind,
) -> Vec<RoadPolyline> {
    let Some(db_path) = db::runtime_db_path(selected_root) else {
        return Vec::new();
    };
    if !db_path.exists() {
        return Vec::new();
    }
    roads_global::load_roads_for_bounds_inner(&db_path, bounds, tile_zoom, layer_kind)
}
