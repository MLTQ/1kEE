# roundtrip.rs

## Purpose
Validates packed vector/contour archives independently of source datasets.
Checks exact feature bytes, negative coordinates, enclosing/crossing geometry,
empty versus missing cells, separate layers/bodies, corruption and read concurrency.
