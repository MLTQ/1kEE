# event_label_layout_tests.rs

## Purpose

Regressions for temporal card placement, independent of rendering or live data.

## Coverage

- Dense clusters and viewport corners fit without collisions when space permits.
- Eight near-coincident moving anchors keep identical slots through 600 frames
  of a ten-second orbit, including fractional text dimensions.
- New events sorting earlier and temporarily missing neighbors do not displace
  established cards; grace-period reservations protect returning cards.
- Edge hysteresis only admits already-visible cards; stale slots expire.
- Resized content preserves neighboring slots, while viewport changes reflow
  the layout into the new bounds.
