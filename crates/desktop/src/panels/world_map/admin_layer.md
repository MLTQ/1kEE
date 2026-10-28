# admin_layer.rs

## Purpose
Load administrative boundary levels from the same Derived/osm, Derived, root/osm
and direct cache roots as other OSM layers. Return immutable background snapshots
with explicit missing/error state instead of silently displaying nothing.

## Components
- `get`: single worker keyed by root and selected levels; no UI-thread disk reads.
- `reload`: invalidates even an empty snapshot and rejects late worker results.
- `AdminData`: regions, missing levels, diagnostic messages for drawer controls.
- `Region`: all rings of one OSM relation, prepared bounds and optional fill mesh.
- `parse_geojson`: supports legacy lines, polygons, multilines and multipolygons.

## Contracts
- Prefer ADMN `.1kc`, then legacy GeoJSON per level; unreadable data can fall back.
- Relation IDs group rings, preserving holes and disconnected islands.
- Only complete polygon-flagged relations or explicit GeoJSON polygons can fill.
  Older binary/LineString caches remain outlines until rebuilt with hole support.
- Invalid coordinates invalidate that relation's fill; errors remain visible.
- Level/visibility/opacity changes never trigger PBF scans. The cache builder
  produces data; Reload reads newly created or replaced files without a restart.

The opt-in admin_preview test renders this loaded cache and controls on a GPU.
