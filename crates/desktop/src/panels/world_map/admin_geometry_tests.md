# admin_geometry_tests.rs

Tests the geographic area of triangulated concave regions, holes and islands;
rejects open lines and checks dateline unwrapping avoids a world-spanning fill.

The dateline fixture aligns an unwrapped outer ring with a hole in the other longitude copy.
