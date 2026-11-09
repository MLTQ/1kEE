# atlas_forest.rs

## Purpose
Read existing forest cells on a worker and prepare coverage for one exact Atlas
height field. Forests become terrain material, sharing its shape, occlusion and
movable lighting instead of bridging ridges as independent elevated polygons.

## Components
- `get`: single-flight snapshot keyed by source root, height-field ID, bounds,
  dimensions and OSM generation. No source scans, downloads or paint-thread I/O.
- `paint_ring`: even-odd scan conversion at pixel centers; closed finite polygons
  only, preserving concavity and short date-line edges. Duplicate source cells
  set coverage idempotently instead of multiplying opacity.
- `Mask`: immutable ID, exact target raster ID, native size and R8 coverage.

## Contracts
One bounded current/pending mask; native height-field limits also bound coverage
memory. Mismatched raster IDs never bind to old/new terrain during handoff.
Toggling visibility only changes material uniforms; cached coverage is reusable.
`clear` rejects late worker publication. GPU readiness is separate from CPU load.

Reads are cell-by-cell, deduplicating exact repeated geometry while retaining different source fragments. A new native field or hidden layer cancels obsolete work between cells. Dateline windows split before source reads.
