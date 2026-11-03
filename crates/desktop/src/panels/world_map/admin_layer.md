# admin_layer.rs

## Purpose
Load administrative boundary levels from the same Derived/osm, Derived, root/osm
and direct cache roots as other OSM layers. Return immutable background snapshots
with explicit missing/error state instead of silently displaying nothing.

## Components
- `get_for`: one worker keyed by source, selected levels, view detail and shading.
- `reload`: invalidates even an empty snapshot and rejects late worker results.
- `AdminData`: nearby regions, fill eligibility, missing levels and diagnostics for the drawer.
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

`get_for` resolves the shared source choice and polls admin_source off-thread.
Snapshot keys include file fingerprints, so completed/replaced caches become
visible automatically even if an earlier load found no files. Explicit roots do
not fall through to an unrelated globally configured Derived directory.

The active camera requests a discrete LOD, padded spatial window and shading mode.
Only selected level fingerprints invalidate a snapshot. Workers read LOD sidecars,
creating them from existing 1KC files when needed; legacy GeoJSON is generalized
in memory. Outlines read spatial pieces. Only shaded requests tessellate complete
rings and compact the mesh to intersecting triangles. The drawer observes the
published snapshot and its precomputed shadeable count without changing its key.
