# atlas_relief_tests.rs

Validate signed-metre TIFF and scaled UInt16 PNG decoding using private temporary
fixtures. Negative ocean heights must retain their sign and PNG quantization
must remain below 0.2 m. No source files or user settings are changed.

The full composed globe shader parses and validates through Naga without needing
a graphics device, including the Atlas uniform and flat Earth background branch.

The local dateline regression checks both ±179.9° focuses: source UVs on both
sides of the image seam, valid UV ranges, and triangle coverage at the real
viewport corners and centre.

The ignored `atlas_real_terrain_preview` now reads a native Europe window with
GDAL, shades actual pixel-edge bounds and renders the production globe shader,
regional raster projection and Appearance controls on a private GPU target.
It optionally exports `ONEKEE_ATLAS_PREVIEW`; no application window is opened.

`native_detail_publishes_in_both_scenes_without_blocking_draw` drives the actual
background request caches for Globe and Local with a 900×900 SRTM window. The
initial calls must return promptly and both asynchronous images must retain their
own exact geographic bounds. This test requires explicit existing terrain data.

The fixed-region preview above is a shading/projection check only. Runtime
selection, desktop tool discovery and actual refined drawing are now covered by
`atlas_runtime_tests`, including a minimal Finder-style PATH.
