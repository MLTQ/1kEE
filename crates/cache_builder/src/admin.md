# admin.rs

## Purpose
Build OSM administrative relations for focused and both whole-planet backends.
Write binary ADMN chunks for levels 2 through 11, including intermediate levels.

## Components
- `load_or_build_admin_boundaries`: focused builds use candidate coordinates.
- `build_planet`: reuses flat or indexed-PBF node lookup; administrative SQLite
  scratch is separate from existing node/way checkpoints.
- Relation/way scans: retain outer, inner and empty-role member ways. Ignore
  label/subarea metadata. Nested geometry relations are omitted because their
  dependencies require recursive resolution.
- `stitch_and_write`: assembles exact node-ID chains with `admin_geometry`, marks
  incomplete relations as outlines, and writes one file per populated level.

## Contracts
- Output: `admin_cells/admin_level_N.1kc`, TAG_ADMN, relation ID in way_id,
  level in class. Each ring is a feature; all rings retain their shared ID.
- Polygon flags are true only for complete relations including inner rings.
- Source identity/importer version invalidates only admin scratch.
- Checkpoints advance after all rows from a blob are saved. Replayed rows are
  idempotent; no mid-blob checkpoint can skip remaining relations or ways.
- Missing focused nodes break lines and disable fill, never invent geometry.
- Completed compact way builds can backfill boundaries without rebuilding ways.

All levels are published, including empty ones, so refreshed sources replace obsolete borders. Checkpoints batch at complete blobs after 256 MiB or a filled record batch.

Compact admin scratch lives under indexed-pbf-v1 with the rest of compact state.

Whole-planet builds replace each level including empty levels. Focused exports
merge by relation: retain existing complete topology over incoming fragments,
keep unrelated regions, and replace fragments once a full relation is available.

Scans iterate only relation/way groups, avoiding expansion of billions of dense
node elements that cannot match. Fresh and resumed scans report bytes/total and
matched records at durable checkpoints. Assembly reports relation/name/count
progress at most once per second, followed by per-level write progress.
