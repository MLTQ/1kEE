# globe_tile_pass_tests.rs

## Purpose
Verify persistent native globe batching without sacrificing contour fidelity.

## Coverage
Byte-for-byte segment multiset equivalence to the previous instance generator,
reuse after neighboring arrivals, theme invalidation, conservative offscreen
culling, and long segments crossing into the viewport.
