# pipelines/mod.rs

Public fuel pipelines occupy FUEL, grid 3, body Earth. Level 0 uses clipped
quarter-degree tiles; level 1 has one simplified world overview at (0,0).
The normal `.1kc` payload retains binary f32 coordinates; its optional name
contains serialized `Info`, not an OSM name. IDs are fragment IDs, never OSM IDs.

## Contracts
- Manifest key `pipelines.manifest.v1` contains full source/license/download
  provenance and import statistics. Missing manifest means unsupported coverage.
- `load` runs on workers; tight views read only intersecting indexed tiles.
  Large views use the baked overview. Empty results do not imply no pipelines exist.
- `decode` checks attribute JSON and finite, bounded coordinates.
- `copy_into` copies only this namespace to a new archive, in a consistent read
  transaction with CRC checks. OSM PIPE and other world layers remain distinct.
- Status, owner vs operator, and unknown/approximate geometry survive packing.

`Filter` independently gates source/product/planned/historical metadata. Unspecified status is visible by default; the UI labels this explicitly.
