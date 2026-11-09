# atlas_relief_tests.rs

Validate signed-metre TIFF and scaled UInt16 PNG decoding using private temporary
fixtures. Negative ocean heights must retain their sign and PNG quantization
must remain below 0.2 m. No source files or user settings are changed.

The full composed globe shader parses and validates through Naga without needing
a graphics device, including the Atlas uniform and flat Earth background branch.
It uses the production desktop shader assembly after the shared-surface split.
A second validation composes the actual OpenXR entrypoint with that same surface
to catch camera/uniform incompatibilities on hosts without an OpenXR runtime.

The ignored `atlas_real_terrain_preview` now reads a native Europe window with
GDAL, retains measured heights and renders the production globe shader,
displaced terrain surface and Appearance controls on a private GPU target.
It optionally exports `ONEKEE_ATLAS_PREVIEW`; no application window is opened.

`native_detail_publishes_in_both_scenes_without_blocking_draw` drives the actual
background request caches for Globe and Local with a 900×900 SRTM window. The
initial calls must return promptly and both asynchronous height fields must retain their
own exact geographic bounds. This test requires explicit existing terrain data.

The fixed-region preview above is a shading/projection check only. Runtime
selection, desktop tool discovery and actual refined drawing are now covered by
`atlas_runtime_tests`, including a minimal Finder-style PATH.

The composed Appearance preview explicitly uses default Atlas lighting, independent of any saved user light settings.

Recent-view tests pin prepared raster reuse/eviction and geographic hysteresis:
small pans retain coverage, but leaving coverage, zooming in, crossing the SRTM
detail threshold, changing roots or clearing the cache requests fresh data.

Retention tests use explicit byte budgets to pin LRU eviction, reject oversized
entries, retain more than eight small windows, reuse covered geographic windows,
and reject different roots/epochs.

Globe residency tests exceed a deliberately tiny LRU budget with three distinct
rasters, revisit the original allocation, contrast Local eviction, and verify
clear prevents both already-running and not-yet-started workers from re-pinning.
