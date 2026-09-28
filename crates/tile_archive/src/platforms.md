# platforms.rs

Owns the validated public offshore installation inventory and independent RIGS
namespace (Earth, grid 4, level/y/x 0) in schema-v1 .1ka archives. Around 9,000
points fit in one small payload; loading once avoids thousands of tile queries.

- `Platform` preserves source IDs, reported status, facility kind, coordinates,
  dates and optional operator/product/depth. No implied live status.
- `Filter` defaults to BSEE + EMODnet, hiding historical, planned and support
  installations. Unknown operating status remains visible and explicitly named.
- `decode` rejects unsupported versions, nonfinite/out-of-range coordinates,
  unknown sources, empty/duplicate IDs, empty/oversized snapshots.
- `load` verifies CRC via Reader and matches the embedded provenance to metadata.
- `build` validates before creating a unique stage; publishes only after SQLite
  closes, refuses replacement and cleans its own intermediate files.
- `copy_into` validates a consistent source snapshot and copies only RIGS and
  its manifest, preserving payload bytes exactly and other world archive namespaces.
- The JSON payload is versioned and limited to 32 MiB / 100,000 records. Raw
  GIS downloads remain outside runtime archives; no raw source is needed to draw.
