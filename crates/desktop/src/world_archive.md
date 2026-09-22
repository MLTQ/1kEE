# world_archive.rs

## Purpose
Discovers Derived/world.1ka and bridges packed archive tiles to desktop loaders.
Runs only in background load workers. Each load owns a read-only connection.

## Contracts
- Vector payloads take priority if the original source cell is absent or its
  fingerprint still matches the packed snapshot. Modified cells use legacy data.
- Corrupt/unsupported archives fall back to existing readers with an error log.
- Contour acceleration currently requires the original contour database for
  manifest/scheduling. Source database/WAL changes disable the packed snapshot.
- Body mapping: Earth=0, Moon=1, Mars=2; contour bucket keys are unchanged.
- Complete source-free terrain mode and archive replacement invalidation while
  running are future work; restart after installing a new archive snapshot.

- Broad vector views (>16 children in a source cell) prefer the existing binary
  cell when available. This prevents the measured full-cell archive regression;
  fine views use packed subtiles. The archive alone can still supply either view.
