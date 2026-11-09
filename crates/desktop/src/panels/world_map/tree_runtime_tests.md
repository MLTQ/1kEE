# tree_runtime_tests.rs

## Purpose
Verify the production Atlas/Trees controllers using existing Alpine terrain and
OSM forest cells at Layer Spread 8, matching the reported failure conditions.

## Contracts
Wait for the real height field and a settled baseline, enable Trees, wait for
complete forest GPU publication, and require changed pixels. The terrain source
ID stays unchanged. Disable Trees and require exact baseline restoration.

Coverage is part of the terrain material and inherits its measured elevations,
lighting, silhouette and occlusion. Generic polygon topology/outline regressions
remain in `tree_layer_tests` for other map themes.

Optional snapshots use `ONEKEE_ATLAS_TREES_BASE_PREVIEW` and
`ONEKEE_ATLAS_TREES_PREVIEW`. Source terrain, forest cells and settings are read-only.
