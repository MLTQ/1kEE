# roundtrip.rs

## Purpose
Validates packed vector/contour archives independently of source datasets.
Checks exact feature bytes, negative coordinates, enclosing/crossing geometry,
empty versus missing cells, separate layers/bodies, corruption and read concurrency.
Inclusive date-line/pole cells round-trip exact geometry and baked heights without
colliding with neighbors; coordinates beyond the geographic endpoints are rejected.
Parallel tests use a process-local sequence in their temporary directory names
so equal clock readings cannot collide.
