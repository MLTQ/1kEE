# marker_style_tests.rs

## Purpose
Verify that resized spires stay interactive and render with the production
camera painter, without accessing map data, settings or live feeds.

## Coverage
- Small/large ground targets and visible beam hits, excluding transparent tips.
- Dense overlapping markers choose the nearest ground anchor before a beam.
- Opt-in GPU test checks actual lit pixels at 25%, 100% and 200%, both upright
  and slanted/selected. `ONEKEE_MARKER_PREVIEW` optionally exports a PNG for review.
