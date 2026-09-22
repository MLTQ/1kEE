# vector.rs

## Purpose
Partitions each existing one-degree vector cell into 64 eighth-degree tiles.
Whole features are assigned by their bounding boxes, preserving every coordinate,
ID, class, flag, name and baked elevation. Readers deduplicate repeated IDs.

## Contracts
- Full detail only (level 0), Earth body 0; v1 does not create simplified LODs.
- Tile assignment is conservative, including touching boundaries and polygons
  that surround a tile without vertices inside it. No clipping or quantization.
- The cell transaction includes empty children, making empty different from
  missing. This asserts snapshot presence, not complete OSM source coverage.
- Queries clamp to the parent cell and visit only overlapping children.
- Source mtime/size are recorded for stale-file detection by desktop readers.

- `prefer_subtiles` chooses packed reads for at most 16 of 64 children when an
  original binary cell is present. Broad views use that cell until baked LODs
  remove the whole-detail reassembly cost. Archive-only reads still work.
- NaN baked-height entries remain bit-exact; runtime sampling handles invalid
  heights. Malformed array lengths and non-finite coordinates are rejected.

- Duplicate feature IDs within a source cell are rejected instead of silently dropping distinct parts while deduplicating child tiles.
