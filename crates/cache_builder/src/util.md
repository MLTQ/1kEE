# util.rs

## Purpose
Shared geographic primitives and helpers for the offline cache-builder. This keeps the builder’s first road-cache command independent from the desktop crate while reusing the same 1° cell model.

## Components

### `GeoPoint`, `GeoBounds`, `RoadPolyline`
- **Does**: Define the normalized geometry types the offline builder works with
- **Interacts with**: `roads.rs`, `geojson.rs`

### `canonical_road_class`
- **Does**: Maps raw OSM `highway=*` values into the normalized major/minor classes used by the road cache
- **Interacts with**: `roads.rs`

### Bounds helpers
- **Does**: Expand bbox searches, test intersection, compute polyline bounds, and map a bbox onto 1° cells
- **Interacts with**: `roads.rs`, `geojson.rs`

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `roads.rs` | road classes match the desktop cache reader’s expectations | Renaming class labels |
| `geojson.rs` | `focus_cells_for_bounds` and `focus_cell_bounds` define the stable 1° cell layout | Changing the cell scheme |

## Notes
- The builder intentionally matches the desktop app’s current direct vector-cell cache layout so the two can interoperate immediately.
