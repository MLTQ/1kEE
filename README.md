# 1kEE

**One Thousand Electric Eye** is a Rust + `egui` desktop OSINT / situational-awareness
surface. A GPU-rendered world globe is the primary canvas, with live event feeds and
public geospatial layers projected onto geography. The core workflow:
event → nearby cameras → attempted feed connection.

## What it does today

This is well past the original "mock-data scaffold." The app integrates a number of
real, live data sources and ships a custom GPU geo-rendering engine:

- **Globe / map canvas** — a custom wgpu fragment shader does per-pixel ray–sphere
  intersection, terrain shading, and anti-aliased graticules for Earth, Moon, and Mars
  (`panels/world_map/`, shader in `panels/world_map/globe.wgsl`).
- **Event stream** — live polling of the Factal API (`factal_stream.rs`), surfaced in
  the brief panel and on the map. Requires an API key in `.1kee_factal_api_key`.
- **Moving tracks** — AIS vessel tracking over AISStream WebSocket (`moving_tracks.rs`)
  and ADS-B flights via OpenSky (`flight_tracks.rs`).
- **Cameras** — public webcam registries and provider scrapers (`camera_registry.rs`,
  `camera_*_catalog.rs`).
- **User geodata** — ArcGIS FeatureServer scraping (`arcgis_source.rs`), plus GeoJSON
  and KML/KMZ layer import (`model/geojson_layer.rs`, `model/kml_layer.rs`).
- **OSM ingest** — planet PBF / Overpass import pipeline producing cached feature cells
  (`osm_ingest/`).
- **Terrain & bodies** — GEBCO / SRTM / Natural Earth elevation, a stellar catalog and
  ephemeris, and a replay timeline.

> **Scope note:** camera-source ingestion stays limited to openly published metadata and
> feeds, subject to source terms and legal review. See [`docs/architecture.md`](docs/architecture.md).

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
boundaries, and [`docs/terrain-pipeline.md`](docs/terrain-pipeline.md) for the GDAL
preprocessing path.

## Run

```bash
# Main app
cargo run -p one-thousand-electric-eye-desktop

# Offline cache builder (GUI)
cargo run -p one-thousand-electric-eye-cache-builder
```

Large raw datasets live under `data/`, derived outputs under `Derived/`; both are
gitignored, as is the Factal API key.

## Notes

- A `puffin` profiler server runs on `127.0.0.1:8585` while the app is up; connect with
  `puffin_viewer`.
- Project planning uses the `beads` (`bd`) issue tracker in `.beads/`.
