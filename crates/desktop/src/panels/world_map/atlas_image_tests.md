# atlas_image_tests.rs

Tests actual raster output for distinct land/water colors, northwest slope
lighting, longitude wrapping, malformed dimensions and transparent invalid
samples. Missing neighbours must not manufacture steep slopes or shadows.

A measured northwest ridge must cast a regional-scale shadow without changing
the valley's central-difference normals. The same pixel array at world scale
cannot cast that shadow, pinning correct physical spacing for native crops.
