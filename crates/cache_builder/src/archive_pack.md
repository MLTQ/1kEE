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
- Before packing, log the vector input count/bytes. This is not an output-size
  estimate: duplication and SQLite indexing can make the archive larger.
- `archive_space` checks the output volume while packing, logs actual temporary
  growth/free capacity and adds a snapshot to errors before staging is removed.
- Vector packing errors identify the source filename and cause. Boundary cells
  at +180 longitude/+90 latitude are valid and must not abort publication.
- Contour databases are opened read-only in a consistent read transaction;
  changed source/WAL metadata aborts conversion. Manifest counts must match rows.
- This is snapshot conversion, not a resumable planet build or complete-world
  coverage claim. The GUI exposes vector packing; mixed contour packing uses the CLI. Incremental repacking is later work.

- Regression covers publication including a nonempty date-line cell, refusing
  an existing destination, and contextual failures/cleanup after invalid input.
