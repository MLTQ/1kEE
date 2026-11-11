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
- **Live transit** — selectable rail and station reports for Seattle/Tacoma, Boston, New York, London, Tokyo Toei, Finland, and Malaysia KTM on the globe and local map. See the [transit guide](docs/live-transit.md).
- **Cameras** — public webcam registries, provider scrapers, and an opt-in
  [Project Eyes On](https://github.com/Y0oshi/Project-Eyes-On)-inspired Insecam
  directory pipeline with feed verification, geolocated map dots, and a
  non-blocking egui live-feed window for snapshots and MJPEG
  (`camera_registry.rs`, `camera_directory_pipeline.rs`,
  `camera_feed_viewer.rs`, `camera_*_catalog.rs`).
- **Follow Events cameras** — shows up to eight camera feeds in the right sidebar,
  ordered by distance from the current event. The list scrolls back to the nearest
  camera at each new stop. Click a preview to open its floating camera window.
  The sidebar also appears in OBS view.
  Adjust stop duration under **Settings → Follow Events → Dwell time**
  (5–300 seconds, default 30); changes apply immediately, and **Save Settings**
  keeps the choice across restarts. Dwell begins after camera arrival.
  Every five live minutes, Follow Events frames the next region and plays
  its past 24 hours of cached events in a one-minute recap. A labeled elapsed-day
  clock and historical chyron accompany the flares; camera feeds and the Brief
  return when live following resumes. All seven continents plus a dedicated
  Middle East zone rotate in about 48 minutes, with broad geographic seams for regional assignment and a
  closer camera zoom tuned for each region.
- **User geodata** — ArcGIS FeatureServer scraping (`arcgis_source.rs`), plus GeoJSON
  and KML/KMZ layer import (`model/geojson_layer.rs`, `model/kml_layer.rs`).
- **Global satellite weather** — enable daily NOAA-20 VIIRS true color and three-hourly
  EUMETSAT infrared under **Earth → Layers → Global Satellite Weather**. Both Globe
  and Local show the latest available frames, with independent opacity controls.
  **Play history** loops up to seven days of imagery with a UTC map clock. Pause or
  scrub to a frame and zoom in to load finer detail for that observation.
  You can also import GeoTIFFs from your own satellite passes. [Layer and receiver guide](docs/sdr-weather-imagery.md)
- **Nearby radio** — find open public-safety and emergency radio near a selected
  event, browse feeds, and listen in a small player. Supports saved public streams
  and native macOS audio ([coverage and usage](docs/emergency-radio.md)).
- **OSM ingest** — planet PBF / Overpass import pipeline producing cached feature cells
  (`osm_ingest/`).
- **Public geospatial layers** — submarine cables and landing points from a
  bundled TeleGeography snapshot (`submarine_cables.rs`, works offline) and live
  NASA FIRMS active-fire detections (`fire_source.rs`, rendered by
  `panels/world_map/fire_layer.rs`, cached locally). Both default to off.
- **GhostMaps events** — enable **Layers → Public Data → GhostMaps events** on the
  Earth map. The default is the rolling 30-day Common Intelligence Picture.
  Use **Choose KMZ exports** to add the 90-day picture or a dated snapshot.
  Each populated KML folder has its own visibility control in Globe and Local.
  While enabled, 1kEE checks the repository catalog once every 24 hours and
  downloads selected files only when their Git blob hashes change or the cached
  copy needs repair. Dated snapshots stay pinned to the chosen filename.
  Validated exports remain available offline in `GhostMaps/layers.sqlite` under
  the data root. [S2 Underground / S2A1, CC BY-NC-SA 4.0](https://github.com/s2underground/GhostMaps)
- **AI data center buildout** — enable **Layers → Public Data → AI data center buildout**.
  1kEE checks [Zack’s Lab](https://www.zackslab.com/buildout) daily while running,
  and keeps the last successful snapshot for offline use. Filled dots mark campus
  locations, rings mark approximate locations, and purple squares mark rules with
  coordinates. Click a marker for details. **Browse buildout** opens
  all campuses, jurisdictional rules and dated news with source links.
- **Atlas appearance** — choose **Settings → Appearance → ATLAS** for cream-paper
  controls, richer red/ochre elevation tints, deep blue bathymetry and lit GEBCO
  3D terrain in Earth Globe and Local views. Heights displace the actual surface;
  optional contours lie on it and disappear behind nearer ridges. Automatically
  refines from original GEBCO terrain, SRTM land elevations and finer cached
  surveys as you zoom. Appearance controls move the light direction/height,
  adjust color richness and relief contrast, or use the Sun with a time offset.
  At sunset at the map center, the controls fade to dark navy over 0.8 seconds.
  At sunrise they fade back to parchment. During Follow Events recaps, the UI follows replay time.
  Warm highlights, cool shadows and broad valley shading follow measured heights;
  moving the light reuses terrain already on the GPU. Save Settings retains these
  choices. The Globe overview remains visible while native detail loads. Local Layer Spread
  controls the surface and contours together. Contours and other layers
  remain independently selectable. Colors represent height/depth, not geology.
  In Local Atlas, Trees applies cached forest coverage directly to the relief,
  sharing its elevations, lighting and occlusion.
- **Terrain & bodies** — GEBCO / SRTM / Natural Earth elevation, the Yale Bright
  Star Catalogue and planetary ephemeris, and a replay timeline.
- **Washington LiDAR** — optional DNR bare-earth and top-surface hillshade in
  Local Earth view, with portal access for survey metadata and downloads
  ([usage](docs/washington-lidar.md)).
- **Gravity anomaly maps** — optional free-air, Bouguer and isostatic residual
  choices with 17 regional NCEI grids in Globe and Local views, prepared on the
  data drive ([coverage and preparation](docs/gravity-anomaly-maps.md)).
- **SWOT marine gravity** — optional offline Scripps SWOT_06 vertical gravity
  gradient in Globe and Local Earth views, revealing seafloor structure alongside
  bathymetry ([data-drive preparation and sources](docs/swot-marine-gravity.md)).
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

For the dual-GPU Arch workstation, `./scripts/run-4090.sh` selects the RTX 4090
and enables larger terrain budgets; see [GPU tuning](docs/gpu-performance.md).

After updating the source, rebuild the desktop app with the command above before
opening the binary directly. Tests build a separate test executable; they do not
update `target/release/one-thousand-electric-eye-desktop`. Close an already-running
app and launch the rebuilt binary to use new features.

The app launches with no configuration: the globe, graticules, and any keyless layers
work out of the box. Live feeds and terrain detail are enabled by configuration below —
everything is optional and independent.

### Native VR on Linux

Start SteamVR, wake the Valve Index, and check that the headset is tracking. Then run the third workspace executable.

```sh
cargo run --release -p one-thousand-electric-eye-vr
```

Raise the left controller to open the wrist controls. Aim with the right controller, click its trigger to select, and use the right stick to scroll. Switch between the stereo globe and the Local terrain table, select Atlas or satellite imagery, browse events, or start Follow Events. Selecting an event opens its brief and a nearby webcam above the map. The VR operator reads the desktop settings and loads geographic tiles as you navigate. See the [VR guide](docs/native-vr.md) for controls, data sources, and timing checks.

### API keys

All keys are entered in-app under **Settings → APIs** and saved locally to
`.1kee_settings.json` next to the binary (gitignored; keys never belong in the repo).

| Layer | Provider | Key required |
| --- | --- | --- |
| Event stream | [Factal](https://www.factal.com/) | Yes — commercial API access |
| Vessels (AIS) | [AISStream](https://aisstream.io/) | Yes — free |
| Webcams | [Windy Webcams](https://api.windy.com/webcams) | Yes — free tier |
| New York traffic cams | [511NY](https://511ny.org/) | Yes — free |
| Public camera directory | Project Eyes On / Insecam | No — explicit opt-in, paced |
| Flights (ADS-B) | [OpenSky](https://opensky-network.org/) | No — anonymous, rate-limited |
| Submarine cables | [TeleGeography](https://www.submarinecablemap.com/) | No — bundled snapshot, no network |
| Active fires | [NASA FIRMS](https://firms.modaps.eosdis.nasa.gov/) | No |

The Project Eyes On adapter is enabled with **Enable live cameras** beside the
top-bar inactive status, or under **Settings → APIs**. It is off by default, accepts
an optional two-letter country scope, and follows the directory's advertised
page total with empty/repeated-page fallbacks. Request starts default to 250/min and are
adjustable under **Settings → APIs**. Recent directory and camera metadata is
reused so the five-minute registry tick does not repeat the full scan. The top
bar reports directory-page, cache, candidate, geolocation, and reachability
progress while it searches. Once the
registry says **live**, enable the Cameras layer and click a green camera pip to
open its feed window. 1kEE ports the public-directory discovery,
deduplication, metadata geolocation, and feed-type verification stages; it does
not include the upstream search-engine dorking path.

1kEE does not seed events or cameras. Until a live source returns records, the
corresponding map layers and lists remain empty.

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
[`docs/cache-builder.md`](docs/cache-builder.md) for whole-planet build measurements
and the complete static-data archive plan. See
[`docs/runtime-archive.md`](docs/runtime-archive.md) to create packed runtime
archives and review measured loading improvements and current limitations. See
[`docs/terrain-pipeline.md`](docs/terrain-pipeline.md) for the GDAL preprocessing path.

## Streaming with OBS

Capture the native 1kEE window in OBS to stream the globe, terrain and live pips.
Use **OBS VIEW** in the map toolbar (or **F10**) for a clean map that keeps any
open Factal brief and live-camera window. **Escape** restores the controls.
Leave it off to capture the full dashboard.
See [OBS setup](docs/obs-streaming.md) for the macOS window-capture steps.

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

Five crates (`cargo` workspace, edition 2024):

- **`crates/desktop`** — the main app (`one-thousand-electric-eye-desktop`): dashboard,
  all live data sources, and the map/globe renderer.
- **`crates/cache_builder`** — offline preprocessing binary
  (`one-thousand-electric-eye-cache-builder`), GUI + CLI subcommands (`roads-bbox`,
  `planet-all`, `contours-bbox`, `pack-archive`) that build road/contour/admin cells and Moon/Mars
  caches from raw OSM/terrain data.
- **`crates/cell_format`** — the custom binary `.1kc` cell format for geographic feature
  cells, with chunk-based forward compatibility.
- **`crates/tile_archive`** — indexed SQLite archives of packed vector and contour
  tiles shared by the builder and desktop.
- **`crates/gruve_sdk`** — vendored Rust client for the GRUVE integration.

See [`docs/architecture.md`](docs/architecture.md) for module layout and integration
boundaries.

## Notes

- `crates/desktop/src/stellar_catalog.rs` is generated. Regenerate it from the
  Yale Bright Star Catalogue with `python3 tools/generate_star_catalog.py`,
  which preserves the curated common names already in the file.
- The submarine cable snapshot in `crates/desktop/src/submarine_cables/` is
  likewise generated: refresh it with `python3 tools/fetch_submarine_cables.py`.
  That data is TeleGeography's, licensed CC BY-NC-SA 3.0 (non-commercial) —
  separate from this repository's code licence.
- Project planning uses the `beads` (`bd`) issue tracker in `.beads/`.
