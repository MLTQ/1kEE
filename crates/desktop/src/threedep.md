# threedep.rs

## Purpose

Acquires USGS 3DEP 1 m bare-earth elevation on demand. The full 1 m holding is
hundreds of terabytes, so this module never mirrors it: it treats
`3DEPElevation/ImageServer` as a clipping service, asks whether 1 m source
exists under a point, and pulls only the small area currently in view.

## Components

### `coverage_at` / `coverage_at_blocking`

- **Does**: Reports whether 3DEP publishes 1 m source under a point, on a 0.05°
  probe grid. The nonblocking form returns `Unknown` on a miss and schedules one
  deduplicated background probe; the blocking form is for workers already off
  the UI thread.
- **Interacts with**: the image service's catalog `query` endpoint,
  `srtm_focus_cache::builders`.
- **Rationale**: A catalog query is a small JSON response, so an uncovered area
  costs no raster download. A bounding-box screen rejects the rest of the world
  before any request is made.

### `fetch_tile_raster`

- **Does**: Downloads one contour tile's elevation raster at exact bounds and
  pixel size, in a single request.
- **Interacts with**: `srtm_focus_cache::gdal::build_threedep_contours`.
- **Rationale**: The service clips and resamples server-side, so a contour tile
  needs no local mosaic. The durable product is the contour geometry in the
  SQLite focus cache; the GeoTIFF is a build temporary.

### Chunk cache (`ensure_chunk`, `peek_chunk`, `chunk_key_for`)

- **Does**: Fetches and caches 0.02° elevation chunks as DEFLATE Float32 COGs
  under `<derived_root>/terrain/3dep_1m/`.
- **Interacts with**: the point-sampling API below.
- **Rationale**: Point sampling needs a local raster, unlike the contour path.
  A fixed grid means neighbouring queries reuse the same download.

### `peek_elevation_m` / `blocking_elevation_m`

- **Does**: 1 m elevation under a point. The peek answers only from an already
  decoded chunk and otherwise schedules the fetch; the blocking form downloads
  and decodes on the calling thread.
- **Interacts with**: `srtm_stream::peek_elevation_m` (marker paint) and
  `srtm_stream::sample_elevation_m` (background layer builders) respectively.
- **Rationale**: The two callers need opposite things. Marker paint must never
  block and can afford to be approximate for a frame. Layer builders bake one
  elevation per vertex into cached geometry, so they need the *same answer for
  the same point every time* — a cache-dependent source writes the difference
  between two terrain models permanently into a road outline.

### `fetch_hillshade_png`

- **Does**: Returns a server-rendered multidirectional hillshade as an 8-bit
  PNG for a bounding box.
- **Interacts with**: `local_terrain_scene::hillshade_layer`.
- **Rationale**: The service shades from its own 1 m source, so this is far
  cheaper and far more detailed than shading a Float32 chunk locally.

### `enforce_cache_budget`

- **Does**: Evicts least-recently-used chunks, with their decoded sidecars,
  until the cache fits the configured ceiling.
- **Interacts with**: `settings_store::threedep_cache_budget_gb`.

## Contracts

| Dependent | Expects | Breaking changes |
|---|---|---|
| `srtm_focus_cache::builders` | `coverage_at` is cheap and never blocks a frame | Making the nonblocking probe synchronous |
| `srtm_stream` (markers) | `peek_elevation_m` never performs network or GDAL work inline | Blocking on a fetch in the peek path |
| `srtm_stream` (layer builders) | `blocking_elevation_m` is deterministic for a given point and coverage | Returning `None` on a lost download race, which silently reverts that vertex to SRTM |
| `hillshade_layer` | `fetch_hillshade_png` returns PNG bytes or `None`, never an error page | Returning the service's HTML error body |
| Cache volume | Total footprint stays under the configured budget | Writing chunks without calling `enforce_cache_budget` |

## Notes

- Measured service limits, as of this integration: `exportImage` fails with an
  HTTP 500 once the encoded response passes roughly 32 MB. `MAX_REQUEST_PX`
  (2400) keeps a Float32 request near 22 MB. A 0.02° chunk is under that at
  every latitude.
- The service reports failures as an HTML page rather than an error status, so
  the TIFF and PNG magic numbers are the only trustworthy success signals.
- 1 m coverage is broader than the "urban only" reputation suggests — rural
  Nevada probes as 1 m — but Alaska is largely 3 m or coarser, and nothing
  outside the United States is served at all.
- `ensure_chunk` waits for another thread's in-flight download of the same
  chunk rather than returning `None`. Giving up would make the result depend on
  download timing, which is exactly the nondeterminism the blocking sampler
  exists to avoid.
- The EHdr driver writes its raw band to exactly the path it is given and
  derives the header by swapping the extension, so decode output must be named
  `.bil`. An extensionless stem silently produces a file nothing can find.
- Live tests covering the probe, tile fetch, chunk round-trip, and hillshade are
  `#[ignore]`d. Run them after changing any request parameter:
  `cargo test -p one-thousand-electric-eye-desktop threedep -- --ignored`
