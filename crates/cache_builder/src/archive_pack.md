# archive_pack.rs

## Purpose
CLI converter from existing binary vector cells and contour caches to a sealed
world.1ka archive. Imports full-detail vector geometry and converts contours to the renderer’s f32 coordinates; never reparses raw PBF/rasters.

## Contracts
- `pack-archive --out FILE` requires at least one source: `--osm-cache-dir DIR`,
  `--earth-contours DB`, `--moon-contours DB`, `--mars-contours DB`.
- Existing destinations are rejected. A unique staging file is atomically
  published via a hard link after SQLite closes; failures clean owned scratch.
- Vector cells are individually fingerprinted before/after conversion.
- Contour databases are opened read-only in a consistent read transaction;
  changed source/WAL metadata aborts conversion. Manifest counts must match rows.
- This is snapshot conversion, not a resumable planet build or complete-world
  coverage claim. The GUI exposes vector packing; mixed contour packing uses the CLI. Incremental repacking is later work.

- Regression covers valid publication, refusing an existing destination, and cleanup after malformed input.
