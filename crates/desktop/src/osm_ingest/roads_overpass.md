# roads_overpass.rs

## Purpose
Focused-road importer for the Overpass fallback path. It is used when the user prefers Overpass or when the local osmium/vector-cache path is unavailable, and it now populates both the legacy SQLite tile store and the direct focused-road vector-cell cache.

## Components

### `import_focus_roads_via_overpass`
- **Does**: Queries the Overpass API for `highway=*` ways in the current focus bounds, parses inline geometry, writes road features into the SQLite tile store, and persists the same roads into the direct per-cell GeoJSON cache
- **Interacts with**: `job_dispatch.rs`, `roads_global.rs`, `roads_vector_cache.rs`
- **Rationale**: Keeps the existing renderer-compatible SQLite path intact while making Overpass-backed revisits as fast as osmium-backed revisits

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `job_dispatch.rs` | Overpass focus jobs return a summary string and update the visible cache state immediately | Removing the direct vector-cache write or changing the summary contract |
| `roads_vector_cache.rs` | Incoming `RoadPolyline` records are valid full polylines that can be merged into per-cell GeoJSON cache files | Changing the road geometry normalization without updating the cache writer |

## Notes
- Overpass is still a focused-region fallback, not a planet bootstrap path.
- The direct vector cache write is merge-based per 1° cell so a new Overpass query does not erase previously cached roads elsewhere in the same cached cell.
