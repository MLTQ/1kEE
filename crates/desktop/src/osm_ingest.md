# osm_ingest.rs

## Purpose
Tracks the planet-scale OSM source, owns the shared runtime schema, and runs background road-ingest jobs against `planet-latest.osm.pbf`. This file exists so the app can treat OSM as a first-class global source without coupling that ingest path to the terrain cache code.

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
- **Rationale**: Lets the app describe “what to build” now even before the full renderer is wired, and gives the worker a clean seam for focused-region priority jobs

### `queue_planet_roads_import`
- **Does**: Queues one explicit global-road bootstrap job against the detected planet source
- **Interacts with**: `terrain_library.rs`, shared job table
- **Rationale**: Planet scans are heavyweight enough that the operator should request them explicitly

### `queue_focus_roads_import`
- **Does**: Queues a high-priority road import centered on the current terrain focus
- **Interacts with**: `terrain_library.rs`, `model.rs`, shared job table
- **Rationale**: Gives the operator visible payoff near the current map focus before the slower global backfill has completed

### `load_roads_for_bounds`
- **Does**: Reads already-imported road polylines back out of the shared SQLite tile store for one viewport bounds + zoom band + layer kind
- **Interacts with**: `local_terrain_scene.rs`, `road_tiles`
- **Rationale**: Keeps rendering decoupled from the ingest worker and lets the map request only the road classes it currently wants to show

### `tick`
- **Does**: Advances the background OSM worker, starts the next queued job, and updates the runtime store asynchronously from the UI thread
- **Interacts with**: `terrain_library.rs`, shared SQLite job table
- **Rationale**: Keeps all planet parsing off the UI thread while still letting the operator inspect and schedule work interactively
- **Notes**: If the app restarts with old jobs still marked `running`, `tick` now recovers those orphaned rows back into a retryable state so stale job metadata does not permanently block fresh focused imports

### `snapshots` / `has_active_jobs`
- **Does**: Exposes the current OSM ingest queue for UI status rendering
- **Interacts with**: `terrain_library.rs`, `header.rs`

### `find_planet_pbf`
- **Does**: Resolves `planet-latest.osm.pbf` from explicit app settings first and otherwise searches under the selected asset root / executable directory
- **Interacts with**: `settings_store.rs`, local filesystem

### `validate_reader`
- **Does**: Confirms that the detected planet file can be opened by the Rust-native `osmpbf` indexed reader
- **Interacts with**: `osmpbf`
- **Rationale**: Establishes the pure-Rust ingest path that can eventually replace any dependence on external OSM tooling

### `supports_locations_on_ways`
- **Does**: Reads the OSM header block and reports whether the source advertises the `LocationsOnWays` optional feature
- **Interacts with**: `BlobReader` in `osmpbf`, `terrain_library.rs`
- **Rationale**: The current pure-Rust global road bootstrap depends on way-embedded coordinates to stay streaming and memory-safe

### `import_focus_roads_via_stream_scan`
- **Does**: Streams the planet PBF directly in Rust, keeps only nodes inside an expanded focused bounds, and writes matching `highway=*` ways into the shared tile store
- **Interacts with**: `osmpbf`, `rusqlite`
- **Rationale**: Focused road jobs should not depend on `LocationsOnWays`, and they should stay inside the app instead of shelling out to slow or wedged external extraction by default

### `import_focus_roads_via_ogr2ogr`
- **Does**: Falls back to a bounded `ogr2ogr` call if the focused Rust scan fails unexpectedly, then writes extracted `highway=*` linework into the shared tile store
- **Interacts with**: app-configured GDAL bin directory or `PATH`, filesystem scratch space, `serde_json`, `rusqlite`
- **Rationale**: Keeps one external escape hatch for focused imports without making it the primary path

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `model.rs` | `detect_from` is cheap enough to run at startup and after root changes | Making inventory expensive or blocking on planet scans |
| `header.rs` | `status_label`, `status_summary`, and `primary_runtime_source` remain concise | Removing or renaming those accessors |
| Future OSM importer | `ensure_runtime_store` creates stable tables for queued jobs plus road/building tiles | Renaming schema without migrating readers/writers |
| `terrain_library.rs` | `tick`, `queue_planet_roads_import`, and `snapshots` remain cheap enough to call every frame / button press | Making them block on heavy import work |

## Notes
- Focused-region road imports are intentionally prioritized above the global bootstrap job.
- The road-layer toggles in the map UI rely on focused imports first: enabling a road layer queues a focused OSM extraction around the current viewport centre and then renders whatever matching roads are already present in the shared tile store.
- Once a road layer is enabled, the map can keep queuing additional focused imports for the current local terrain focus / viewport as the operator moves, without touching the unfinished global planet bootstrap path.
- The runtime store is separate from terrain because the OSM ingest path will need different tile semantics, metadata, and job scheduling than the contour cache.
- `queue_region_job` is still the seam for future work: current-focus road/building prefetch can insert jobs here long before the renderer consumes those layers.
- `validate_reader` uses the Rust-native `osmpbf` stack so the eventual “download the planet file and it just works” workflow does not depend on an external `osmium-tool` install.
- The current pure-Rust global bootstrap is intentionally gated on `LocationsOnWays`. If the source PBF does not advertise that optional feature, the global job will fail explicitly instead of trying to build a giant node dependency map in memory.
- Focused road imports now use a bounded in-process Rust scan first and only fall back to `ogr2ogr` if that scan fails, which makes the road-layer workflow far less dependent on external GDAL behavior.
- The worker only allows one live import thread, so multiple `running` rows in the DB always indicate stale state from an older app session; those are now recovered automatically the next time `tick` runs.
- The importer no longer assumes a specific Postgres.app install path for `ogr2ogr`; that tool is now resolved from the app settings or the ambient `PATH`.
