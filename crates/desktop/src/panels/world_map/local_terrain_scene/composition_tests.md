# composition_tests.rs

## Purpose
Regression fixtures reproduce the stuck-coarse-map case with a missing finer
edge cell, plus a full out/in reversal. No data drive or network is needed.

## Contracts
Ready fine pieces appear immediately in the composition; old lines survive
only outside those pieces. Cuts insert endpoints and never bridge gaps. Empty
decoded cells replace old geometry; missing cells do not. Tests verify stable
Arc reuse, area conservation, disjoint rectangles and legacy clipping at
negative Mexico coordinates.
