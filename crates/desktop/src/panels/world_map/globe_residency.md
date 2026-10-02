# globe_residency.rs

## Purpose
Restrict Earth globe source tiles and composed terrain to the current viewport.
The previous wide geographic cap bounded tile count but retained offscreen
geometry and allowed old zoom snapshots to grow during event tours.

## Components
- `Viewport` tests a conservative sphere around each geographic patch against
  the actual globe projection's screen, hemisphere and near clipping planes.
  Curved edges and date-line crossings cannot disappear from corner-only tests.
- `Residency` tracks the visible core set within the bounded source window,
  reader-measured bounds and a revision that changes only at core transitions.
- `prune` evicts source Arcs; late reads consult the same latest residency.

## Contracts
Bounds scans occur on readers/workers. Paint tests small metadata only. Legacy
halos remain eligible if they can contribute visible coverage. Composition and
fallback clipping use the same visible regions; old zooms never retain an entire
travel history. Invalid transforms conservatively preserve coverage.
