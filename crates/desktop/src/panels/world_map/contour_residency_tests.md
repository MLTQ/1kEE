# contour_residency_tests.rs

## Purpose
Exercise screen-space contour residency without source data or global caches.

## Coverage
- Crossing segments survive even with both endpoints offscreen.
- Elevation can move ground outside the viewport back onto the screen.
- Rotated bounding boxes use separating axes, not only a screen AABB.
- Panning releases decoded geometry; late readers cannot resurrect it.
- Bounds metadata prevents repeat offscreen reads and allows a return-pan reload.
- Empty-tile coverage and metadata retention remain bounded.
- Culled disk tiles complete loading progress without restoring their geometry.

## Contracts
Tests use isolated cache instances and synthetic geometry. No Hilbert files,
network providers or live desktop state are involved.
