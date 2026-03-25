# admin.rs — OSM Administrative Boundary Builder

## Purpose

Implements two extra PBF scan passes (Pass R and Pass A) plus a stitching step to
produce per-admin-level GeoJSON files from OSM `boundary=administrative` relations.

## Pass order

| Pass | What it does |
|------|--------------|
| Pass R (relation scan) | Iterates all blobs; saves admin relations + member way IDs to SQLite. |
| Pass A (admin way scan) | Iterates all blobs; saves ordered node refs for every admin member way. |
| Stitch + write | Resolves coords, stitches ways into chains, writes `admin_cells/admin_level_N.geojson`. |

## Public API

### `load_or_build_admin_boundaries`

```rust
pub fn load_or_build_admin_boundaries(
    command: &BboxCommand,
    bounds: GeoBounds,
    node_store: &mut NodeStore,
    progress: &mut dyn FnMut(RoadBuildProgress),
) -> Result<usize, String>
```

Called from `roads.rs` after the way scan when `command.build_admin` is true.
Returns the total number of GeoJSON Feature objects (rings) written.

## Output format

Files are written to `{cache_dir}/admin_cells/admin_level_{N}.geojson` where N ∈ {2, 4, 6, 8}.

Each Feature has:
- `geometry.type = "LineString"`
- `geometry.coordinates = [[lon, lat], ...]`
- `properties.relation_id`
- `properties.name`
- `properties.admin_level`

## Checkpointing

Both scans use the `build_state` table with keys `scan_offset_relation_scan` and
`scan_offset_admin_way_scan` so interrupted runs can resume. Completion flags
`relation_scan_complete` and `admin_way_scan_complete` prevent redundant re-runs.

## Stitching

`stitch_ways` uses a greedy chain algorithm: it builds an endpoint index keyed by
`(lat × 1e5, lon × 1e5)` integer pairs, then grows chains by matching the current
tail against unused ways' start or end points, reversing ways as needed.
