# osm_ingest.rs

## Purpose
Tracks the planet-scale OSM source, owns the shared runtime schema, and runs the first background road-bootstrap jobs against `planet-latest.osm.pbf`. This file exists so the app can treat OSM as a first-class global source without coupling that ingest path to the terrain cache code.

## Components

### `OsmInventory`
- **Does**: Summarizes whether the planet PBF is present, whether the shared runtime SQLite store is initialized, and how many queued jobs / derived tile manifests exist
- **Interacts with**: `model.rs`, `header.rs`

### `ensure_runtime_store`
- **Does**: Creates `Derived/osm/osm_runtime.sqlite`, enables WAL mode, creates the shared schema, and records the detected planet source metadata
- **Interacts with**: local filesystem, `rusqlite`, `terrain_assets.rs`
- **Rationale**: Keeps the eventual road/building cache in one indexed store rather than a directory of tiny files

### `queue_region_job`
- **Does**: Inserts a queued ingest request for one geographic bounds + feature kind into the shared runtime DB
- **Interacts with**: future background OSM importer, `rusqlite`
- **Rationale**: Lets the app describe “what to build” now even before the full planet extraction worker is wired

### `queue_planet_roads_import`
- **Does**: Queues one explicit global-road bootstrap job against the detected planet source
- **Interacts with**: `terrain_library.rs`, shared job table
- **Rationale**: Planet scans are heavyweight enough that the operator should request them explicitly

### `tick`
- **Does**: Advances the background OSM worker, starts the next queued job, and updates the runtime store asynchronously from the UI thread
- **Interacts with**: `terrain_library.rs`, shared SQLite job table

### `snapshots` / `has_active_jobs`
- **Does**: Exposes the current OSM ingest queue for UI status rendering
- **Interacts with**: `terrain_library.rs`, `header.rs`

### `find_planet_pbf`
- **Does**: Resolves `planet-latest.osm.pbf` from the selected root, its ancestors, the repo `Data/` tree, or the external Hilbert volume
- **Interacts with**: local filesystem

### `validate_reader`
- **Does**: Confirms that the detected planet file can be opened by the Rust-native `osmpbf` indexed reader
- **Interacts with**: `osmpbf`
- **Rationale**: Establishes the pure-Rust ingest path that can eventually replace any dependence on external OSM tooling

### `supports_locations_on_ways`
- **Does**: Reads the OSM header block and reports whether the source advertises the `LocationsOnWays` optional feature
- **Interacts with**: `BlobReader` in `osmpbf`, `terrain_library.rs`
- **Rationale**: The current pure-Rust global road bootstrap depends on way-embedded coordinates to stay streaming and memory-safe

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `model.rs` | `detect_from` is cheap enough to run at startup and after root changes | Making inventory expensive or blocking on planet scans |
| `header.rs` | `status_label`, `status_summary`, and `primary_runtime_source` remain concise | Removing or renaming those accessors |
| Future OSM importer | `ensure_runtime_store` creates stable tables for queued jobs plus road/building tiles | Renaming schema without migrating readers/writers |
| `terrain_library.rs` | `tick`, `queue_planet_roads_import`, and `snapshots` remain cheap enough to call every frame / button press | Making them block on heavy import work |

## Notes
- The current importer only handles the first global roads bootstrap path.
- The runtime store is separate from terrain because the OSM ingest path will need different tile semantics, metadata, and job scheduling than the contour cache.
- `queue_region_job` is still the seam for future work: current-focus road/building prefetch can insert jobs here long before the renderer consumes those layers.
- `validate_reader` uses the Rust-native `osmpbf` stack so the eventual “download the planet file and it just works” workflow does not depend on an external `osmium-tool` install.
- The current pure-Rust global bootstrap is intentionally gated on `LocationsOnWays`. If the source PBF does not advertise that optional feature, the job will fail explicitly instead of trying to build a giant node dependency map in memory.
