# 1kEE Architecture

## Purpose
This document captures the structure of the 1kEE desktop app as it stands. It now spans
a GPU geo-renderer, several live data connectors, an OSM ingest pipeline, and a custom
on-disk cell format — well beyond the original mock-data MVP. Keep this file in sync as
modules move.

## Analyst flow

1. Render the global operations canvas (GPU globe, terrain, layers).
2. Surface live events (Factal) with severity and location.
3. Let an analyst select an event.
4. Show geolocated camera pips from the normalized registry.
5. Open a selected snapshot/MJPEG feed in a separate egui window.

Live tracks (vessels, flights) and user-imported geodata (ArcGIS / GeoJSON / KML) layer
onto the same canvas, with a replay timeline over the event history.

## Crates

- **`crates/desktop`** — the application. Entry `main.rs` → `DashboardApp` (`app.rs`),
  which owns the `AppModel`, ticks each data source once per frame, and renders panels.
- **`crates/cache_builder`** — offline preprocessing (GUI in `app.rs`, CLI in
  `args.rs`/`main.rs`). Builds road/contour/admin feature cells and Moon/Mars/planet
  caches from raw OSM and terrain inputs.
- **`crates/cell_format`** — the `.1kc` binary cell format (`lib.rs`): a chunked,
  forward-compatible container for per-cell geographic features (roads, water,
  buildings, power, rail, admin, …). Shared by the builder (writer) and desktop (reader).

## Desktop module layout

- `model/` — domain + UI state. `model/mod.rs` holds the central `AppModel`; submodules
  cover `events`, `cameras`, `vessels`, `flights`, `arcgis`, `geojson_layer`,
  `kml_layer`, `replay`, and shared `geo` types.
- `panels/` — egui panels: `header`, `layer_drawer`, `event_list`, `camera_list`,
  `factal_brief`, `factal_settings`, `terrain_library`, `stellar_observatory`,
  `replay_controls`, `status_log`, and `world_map`.
- `panels/world_map/` — the renderer. `globe_pass.rs` owns the wgpu pipeline and the
  `GlobeUniforms` contract; the WGSL lives in `globe.wgsl`. `globe_scene/` and
  `local_terrain_scene/` drive the two view modes; per-feature layers (road, water,
  building, power, infra, admin, tree, contour, stellar, graticule) draw on top.
- `osm_ingest/` — OSM import (osmium / Overpass / streaming, SQLite stores, job
  dispatch).
- `gruve/` — Gruve mesh integration (companion web view + shared state). An embedded
  HTTP server publishes a per-frame `Snapshot` of `AppModel` and serves a web globe
  that mirrors the host view; viewers can steer the host or drop shared pins. Owned by
  `GruveBridge`, driven from the frame loop. See `gruve/mod.md`.
- Data-source / service modules at the crate root: `factal_stream`, `moving_tracks`
  (AIS), `flight_tracks` (ADS-B), `arcgis_source`, `camera_registry`,
  `camera_*_catalog`, `city_catalog`, `stellar_catalog`, `planet_ephemeris`,
  `stellar_time`, `terrain_assets`, `terrain_precompute`, `event_store`,
  `settings_store`, `theme`.

### Frame loop

`DashboardApp::update` (`app.rs`) ticks the live sources (`factal_stream::tick`,
`camera_registry::tick`, replay/stellar advance), then renders panels and the central
`world_map`. Background worker threads wake the UI through a global
`OnceLock<egui::Context>` (`app::request_repaint`); each source exposes a `shutdown()`
called from `DashboardApp::drop`.

### Globe GPU contract

`GlobeUniforms` (Rust, `globe_pass.rs`) and the `Uniforms` struct (WGSL, `globe.wgsl`)
must match byte-for-byte at a fixed 128-byte layout. A `const _: () = assert!(...)`
size guard in `globe_pass.rs` turns a desynced layout into a compile error; the offset
table above the struct is the source of truth for both sides.

## Integration boundaries

### Event ingest (Factal)
- Input: Factal API event records (title, severity, timestamp, summary, coordinates).
- Output: normalized events in `AppModel`, persisted to the event store for replay.
- Open work: retry/backoff tuning, dedupe, TTL/aging (see `.beads`).

### Camera registry
- Input: openly published camera metadata and feed URLs (registries + provider scrapers),
  including an explicit opt-in, bounded Project Eyes On-compatible Insecam directory adapter.
- Output: normalized camera records with provider, location, type, reachability.
- Progress: the background registry reports directory pages, candidates,
  geolocated cameras, and reachable feeds to a non-blocking top-bar indicator.
- Interaction: clicking a map pip starts a bounded background image/MJPEG reader;
  the UI thread only uploads the latest decoded frame to an egui texture.
- Open work: more provider adapters, geocoding, provenance, health checks, legal flags.

### Map / globe layer
- Custom GPU globe (ray-traced sphere, terrain shading, graticules) for Earth/Moon/Mars,
  plus a local high-resolution terrain scene and OSM-derived feature cells.
- Constraint: keep event/camera hit-testing deterministic and simple for analysts.

### Gruve mesh integration
- Input: the live `AppModel` (read-only snapshot per frame) and validated commands
  from web viewers (Focus / SelectEvent).
- Output: a discoverable lobby tile (announce, L1), an `api` upstream serving the
  live picture as JSON (L2), and a shared session for pins/cursors (L3). Conformance
  is checked by `gruve doctor` against `crates/desktop/src/gruve/web`.
- Constraint: never block or panic into the host; publish camera *positions and
  reachability*, not feed URLs; remote commands drive the store, never replay input.
- Open work: publish host view into the session for richer late-joiner replay; serve
  a coastline layer in the companion globe; optional `service: true` data feed.

## Safety / scope notes

- Real feed connectors should stay inside an allowlisted, provenance-aware adapter layer.
- The Project Eyes On integration intentionally excludes broad search-engine dorking;
  it accepts only public-IP feed URLs advertised by the configured directory and rejects
  loopback, LAN, link-local, credentialed, documentation, and reserved targets.
- Source-specific terms, licensing, and jurisdictional constraints must be tracked
  before enabling any live ingestion path.
- The Factal API key lives in `.1kee_factal_api_key` (gitignored) — keep secrets out of
  the tree.
