# admin_layer

## Purpose

Loads and caches OSM administrative boundary geometries from pre-built GeoJSON
files written by the cache builder.  Exposes a single function that returns all
boundaries for a set of admin levels, loading from disk only when the active
cache root changes.

## Components

| Symbol | Kind | Description |
|---|---|---|
| `LoadedAdminBoundary` | struct | A single boundary polyline with its OSM relation ID, admin level, optional name, and projected lat/lon points. |
| `load_admin_boundaries` | fn | Reads `{cache_root}/admin_cells/admin_level_{level}.geojson` for each requested level, parses LineString features, and returns a flat list of boundaries. |
| `get_or_load_admin_boundaries` | fn | Session-scoped cache wrapper.  Calls `load_admin_boundaries` once per distinct `cache_root`; returns cloned data on subsequent calls. |
| `ADMIN_CACHE` | static | `OnceLock<Mutex<AdminCache>>` — holds the last-loaded root path and the parsed boundaries. |

## Contracts

- Files are expected at `{cache_root}/admin_cells/admin_level_{level}.geojson`
  where `level` ∈ {2, 4, 6, 8}.  Missing files are silently skipped.
- Each file must be a GeoJSON `FeatureCollection` whose features have
  `geometry.type = "LineString"` and properties `relation_id` (i64),
  `name` (string or null), `admin_level` (number).
- `get_or_load_admin_boundaries` is called on the render thread; the mutex
  ensures thread-safety without background loading (the files are small enough
  to load synchronously on first use).
- If the cache root changes (user switches terrain library), the cache is
  invalidated automatically and reloaded.

## Notes

- Admin boundaries are not cell-streamed — they cover the whole world for each
  level and are loaded all at once.
- The `GeoPoint` type is `crate::model::GeoPoint` (lat/lon in degrees, f32).
- Rendering lives in `local_terrain_scene/mod.rs`; colours and stroke widths
  come from `crate::theme::admin_color` and `crate::theme::admin_stroke_width`.
- Level 2 (country) is drawn last so it sits on top of finer-grained levels.
