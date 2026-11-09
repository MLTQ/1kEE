# atlas_relief_tests.rs

Validate signed-metre TIFF and scaled UInt16 PNG decoding using private temporary
fixtures. Negative ocean heights must retain their sign and PNG quantization
must remain below 0.2 m. No source files or user settings are changed.

The full composed globe shader parses and validates through Naga without needing
a graphics device, including the Atlas uniform and flat Earth background branch.

The local dateline regression checks both ±179.9° focuses: source UVs on both
sides of the image seam, valid UV ranges, and triangle coverage at the real
viewport corners and centre.

The ignored `atlas_real_terrain_preview` reads the existing GEBCO overview from
`ONEKEE_ATLAS_ROOT`, shades it with production `HeightGrid::image`, and renders
the production globe shader/raster projection plus Appearance controls on a
private GPU target. It verifies visible blue ocean and optionally exports
`ONEKEE_ATLAS_PREVIEW`. No application window is opened.
