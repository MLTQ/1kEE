# deflock_source.rs

## Purpose

Provides a cache-first, public-data-only import adapter for DeFlock-compatible
OpenStreetMap ALPR locations. It supplies immutable location snapshots to the
UI model without owning map rendering or collecting user data.

## Components

### `DeflockAlprLocation`
- **Does**: Represents public OSM ALPR geometry and selected public metadata
  needed for transparent map display and attribution.
- **Interacts with**: `AppModel` and the world-map Deflock overlay.

### `tick` / `shutdown`
- **Does**: Schedule cache reads and bounded Overpass refresh work without
  blocking the UI, and retire worker results during app shutdown.
- **Interacts with**: `app.rs` and `AppModel`.

### Provenance constants
- **Does**: Provide DeFlock and OpenStreetMap attribution strings/links for UI
  surfaces that expose the public source.
- **Interacts with**: map/source status presentation.

### Cache and Overpass parser helpers
- **Does**: Load a local snapshot first, validate the canonical DeFlock GeoJSON
  or tag-checked Overpass JSON, and persist a successful public-data snapshot
  atomically for offline reuse.
- **Interacts with**: the effective asset root, `reqwest`, and `serde_json`.

### Refresh policy

- **Does**: Uses the local `Data/deflock_alpr_locations.json` snapshot (or the
  configured Data Root) immediately, refreshes snapshots at most every 12
  hours, and exponentially backs off failed requests from five minutes to two
  hours.
- **Interacts with**: the canonical DeFlock public snapshot, primary/mirror
  OpenStreetMap Overpass ALPR queries, and the layer-drawer status text.
- **Rationale**: The app remains usable offline and never performs HTTP work on
  the render/UI thread.

## Contracts

| Dependent | Expects | Breaking changes |
|---|---|---|
| `AppModel` | Location snapshots contain only valid public coordinates and stable OSM identity | Changing ID/coordinate fields or including private data |
| `app.rs` | `tick` is non-blocking and `shutdown` prevents stale result application | Performing HTTP or disk scans on the UI thread |
| World-map renderers | Input is immutable public location/direction metadata, with no fetch ownership | Moving rendering or mutable cache state into the layer |

## Notes

- The display toggle defaults off and empty/offline cache states remain usable.
- Attribution must identify OpenStreetMap contributors and DeFlock without
  implying an official affiliation or real-time guarantee.
- The importer queries only public OSM `man_made=surveillance` /
  `surveillance:type=ALPR` node and way metadata. It does not import live feeds
  or provide routing/avoidance behavior.
- Empty or malformed source/cache responses are rejected rather than replacing
  a usable snapshot with a fresh empty cache.
