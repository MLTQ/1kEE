# 1kEE

**One Thousand Electric Eye** is a Rust + `egui` desktop OSINT / situational-awareness
surface. A GPU-rendered world globe is the primary canvas, with live event feeds and
public geospatial layers projected onto geography. The core workflow:
event → geolocated camera pips → live snapshot/MJPEG feed window.

<!-- Screenshots: drop 1–2 images here, e.g.
![1kEE globe view](docs/media/globe.png)
-->

## Features

- **Globe / map canvas** — a custom wgpu fragment shader does per-pixel ray–sphere
  intersection, terrain shading, and anti-aliased graticules for Earth, Moon, and Mars
  (`panels/world_map/`, shader in `panels/world_map/globe.wgsl`).
- **Event stream** — live polling of the Factal API (`factal_stream.rs`), surfaced in
  the brief panel and on the map.
- **Moving tracks** — AIS vessel tracking over AISStream WebSocket (`moving_tracks.rs`)
  and ADS-B flights via OpenSky (`flight_tracks.rs`).
- **Cameras** — public webcam registries, provider scrapers, and an opt-in
  [Project Eyes On](https://github.com/Y0oshi/Project-Eyes-On)-inspired Insecam
  directory pipeline with feed verification, geolocated map dots, and a
  non-blocking egui live-feed window for snapshots and MJPEG
  (`camera_registry.rs`, `camera_directory_pipeline.rs`,
  `camera_feed_viewer.rs`, `camera_*_catalog.rs`).
- **User geodata** — ArcGIS FeatureServer scraping (`arcgis_source.rs`), plus GeoJSON
  and KML/KMZ layer import (`model/geojson_layer.rs`, `model/kml_layer.rs`).
- **OSM ingest** — planet PBF / Overpass import pipeline producing cached feature cells
  (`osm_ingest/`).
- **Terrain & bodies** — GEBCO / SRTM / Natural Earth elevation, a stellar catalog and
  ephemeris, and a replay timeline.
- **Multiplayer** — a companion web view that mirrors the analyst's live view onto a
  local mesh (see below).

> **Scope note:** camera-source ingestion stays limited to openly published metadata and
> feeds, subject to source terms and legal review. See [`docs/architecture.md`](docs/architecture.md).

## Getting started

You need a recent stable Rust toolchain (the workspace uses edition 2024) and a GPU
with Metal / Vulkan / DX12 support for wgpu.

```bash
cargo run --release -p one-thousand-electric-eye-desktop
```

The app launches with no configuration: the globe, graticules, and any keyless layers
work out of the box. Live feeds and terrain detail are enabled by configuration below —
everything is optional and independent.

### API keys

All keys are entered in-app under **Settings → APIs** and saved locally to
`.1kee_settings.json` next to the binary (gitignored; keys never belong in the repo).

| Layer | Provider | Key required |
| --- | --- | --- |
| Event stream | [Factal](https://www.factal.com/) | Yes — commercial API access |
| Vessels (AIS) | [AISStream](https://aisstream.io/) | Yes — free |
| Webcams | [Windy Webcams](https://api.windy.com/webcams) | Yes — free tier |
| New York traffic cams | [511NY](https://511ny.org/) | Yes — free |
| Public camera directory | Project Eyes On / Insecam | No — explicit opt-in, bounded |
| Flights (ADS-B) | [OpenSky](https://opensky-network.org/) | No — anonymous, rate-limited |

The Project Eyes On adapter is enabled with **Enable live cameras** beside the
top-bar Demo status, or under **Settings → APIs**. It is off by default, accepts
an optional two-letter country scope, and fresh configurations scan the bounded
maximum of five directory pages per poll. The top bar reports directory-page,
candidate, geolocation, and reachability progress while it searches. Once the
registry says **live**, enable the Cameras layer and click a green camera pip to
open its feed window. 1kEE ports the public-directory discovery,
deduplication, metadata geolocation, and feed-type verification stages; it does
not include the upstream search-engine dorking path.

### Terrain & map data

Detailed terrain (contours, shaded relief, roads) comes from locally preprocessed
datasets — GEBCO bathymetry, SRTM elevation, Natural Earth relief, and OSM extracts.
Raw downloads live under a `data/` directory and derived outputs under `Derived/`;
both are gitignored. The offline cache builder turns raw data into runtime assets:

```bash
# GUI, or CLI subcommands: roads-bbox, planet-all, contours-bbox
cargo run --release -p one-thousand-electric-eye-cache-builder
```

Point the app at your asset root under **Settings → Paths**. See
[`docs/terrain-pipeline.md`](docs/terrain-pipeline.md) for the GDAL preprocessing path.

## Multiplayer over Gruve

The app can put itself on a Gruve mesh (a local-network app-sharing/collaboration
layer, not yet public) via a companion web view (`crates/desktop/src/gruve/`). Because 1kEE is a native egui/wgpu app it can't be
served over the mesh directly, so the running app embeds a small HTTP server that:

- **announces** itself to the local Gruve agent (a `1kEE` tile appears in the lobby),
- serves a thin web globe that **mirrors the analyst's live view** — host camera centre,
  events, vessels, flights, cameras, and the active theme, polled from the app, and
- lets a viewer **steer the host** ("look here" / select an event) and drop a **shared
  pin** that every viewer of the tile sees (Gruve session state).

It degrades silently: with no agent running, no free port, or no viewers, the desktop
app behaves exactly as before. The feed-connection step stays on the host — camera
*positions and reachability* go to the mesh, not feed URLs. The Gruve Rust SDK is
vendored in `crates/gruve_sdk` and the JS SDK in the desktop crate's web assets, so
the build has no external Gruve dependency; the Gruve agent itself is separate tooling
and not part of this repo.

## Workspace

Three crates (`cargo` workspace, edition 2024):

- **`crates/desktop`** — the main app (`one-thousand-electric-eye-desktop`): dashboard,
  all live data sources, and the map/globe renderer.
- **`crates/cache_builder`** — offline preprocessing binary
  (`one-thousand-electric-eye-cache-builder`), GUI + CLI subcommands (`roads-bbox`,
  `planet-all`, `contours-bbox`) that build road/contour/admin cells and Moon/Mars
  caches from raw OSM/terrain data.
- **`crates/cell_format`** — the custom binary `.1kc` cell format for geographic feature
  cells, with chunk-based forward compatibility.

See [`docs/architecture.md`](docs/architecture.md) for module layout and integration
boundaries.

## Notes

- A `puffin` profiler server runs on `127.0.0.1:8585` while the app is up; connect with
  `puffin_viewer`.
- Project planning uses the `beads` (`bd`) issue tracker in `.beads/`.
