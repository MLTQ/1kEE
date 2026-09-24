# lib.rs

## Purpose
Shared indexed archive for packed static map tiles. Encoding stays separate from
SQLite storage. The builder writes snapshots; desktop workers open independent
read-only connections, never a shared connection mutex.

## Contracts
- Schema v1 uses body/layer/grid/level/y/x keys. Vector grid 1 divides degrees
  into eighths; contour grid 2 preserves the existing terrain bucket scheme.
- Vector source cells commit all 64 child tiles in one transaction. Empty tile
  payloads are present; absent source cells mean unknown coverage. Presence only
  describes the imported cache snapshot, not completeness of the original OSM.
- New archives are built at a unique temporary path and published by the CLI
  after close. Published archives are snapshots; readers never mutate them.
- Payload CRC32 checksums and decoder bounds reject corruption. Vector bytes
  retain source precision; contours store the renderer's existing f32 precision.
- `world.1ka` is the conventional filename under Derived. Actual format is SQLite.

## Components
- `Reader`: validates schema and performs indexed tile/cell queries.
- `Writer`: writes tile batches transactionally and seals the database.
- `vector`: partitions full vector features into smaller read units.
- `contours`: converts contour rows to renderer-coordinate arrays in one tile payload.

- contour_grid and contour_clip share Earth core ownership, aligned source halos
  and strict f64 GeoPackage clipping across both builders. Existing archive
  readers and CTF1 payloads remain compatible.
