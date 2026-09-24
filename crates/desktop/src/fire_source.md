# fire_source.rs

## Purpose

Provides a cache-first, public-data-only adapter for NASA FIRMS active-fire
detections (VIIRS thermal anomalies, last 24 hours). It supplies immutable
detection snapshots to the UI model without owning map rendering.

## Components

### `FireDetection` / `FireConfidence`
- **Does**: Represent one VIIRS fire pixel — position, fire radiative power,
  reported confidence, acquisition time, and satellite.
- **Interacts with**: `AppModel` and `panels/world_map/fire_layer.rs`.

### `tick` / `shutdown`
- **Does**: Schedule cache reads and a bounded hourly refresh off the UI
  thread, and retire worker results during app shutdown.
- **Interacts with**: `app.rs` and `AppModel`.

### `step`
- **Does**: Advances the source state machine one frame under the lock: records
  a finished outcome (backoff, next attempt), *then* decides whether to start
  the next worker. Pure over `SourceState`, so it is unit-tested directly.
- **Interacts with**: `tick`, `apply_outcome`.

### `parse_csv`
- **Does**: Parse a FIRMS CSV product, resolving columns **by header name**
  rather than position, and drop low-confidence rows.
- **Interacts with**: the two VIIRS endpoints and the local cache.

### Cache helpers (`cache_dir`, `read_cache`, `write_cache`)
- **Does**: Resolve the cache location the same way `deflock_source` does, and
  persist the concatenated products through a temporary file.
- **Interacts with**: `settings_store`, the effective asset/data root.

## Contracts

| Dependent | Expects | Breaking changes |
|---|---|---|
| `AppModel` | Detections carry finite, in-range coordinates and non-negative FRP | Emitting unvalidated coordinates or negative/NaN FRP |
| `app.rs` | `tick` is non-blocking and does nothing while the layer is off | Performing HTTP or disk work on the UI thread |
| `fire_layer` | Input is immutable detection metadata, with no fetch ownership | Moving rendering or mutable cache state into the source |
| Event surfaces | Fires never enter `event_store` or the brief | Routing detections into `EventRecord` without clustering first |

## Notes

- **These are a map layer, not events.** A global VIIRS day is ~60,000
  detections; feeding them into the event stream would bury the Factal/USGS
  brief and the event store under agricultural burning. Emitting fire *events*
  would first require spatial clustering into incidents.
- Low-confidence detections are dropped at parse time (~10% of rows). They are
  the noisiest tier — sun glint, hot soil, gas flares — and carry the least
  analytic value.
- Columns are resolved by header name because the MODIS and VIIRS products
  order them differently and FIRMS has added columns before.
- Both satellites are merged; their orbits interleave, roughly doubling revisit
  coverage. One satellite failing still leaves a usable, thinner layer.
- The display toggle defaults off, and nothing is fetched until it is turned
  on. A stale cache is preferred over an empty layer when the network fails.
- **Record before you decide.** An earlier `tick` chose whether to spawn a
  worker before recording the result that had just arrived. With nothing yet
  scheduled it started a new request and bumped the generation, which made the
  fresh result look stale — so every result was discarded and the layer loaded
  forever. `step` does both in the right order by construction, and
  `an_arriving_result_is_kept_not_discarded` fails if the order is reversed.
- Workers call `crate::app::request_repaint()` on completion, so results show
  up without waiting for the next input event.
- Verified against a live product: 59,609 rows → 53,706 retained, 2,936 high
  confidence, peak FRP 560 MW.
