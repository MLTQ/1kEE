# event_chyron_tests.rs

## Purpose
Verify the real egui news crawl at fixed times without live data or the desktop.

## Coverage
- Five distinct rows contain complete title, level and summary at narrow,
  minimum-window and wide widths; each has one line and its own clip rectangle.
- Overflow holds initially, scrolls left at the intended speed, and reaches the
  final words at the right edge before the story can be replaced.
- Camera selection changes, reordered polls and revised metadata retain ongoing
  row text/start times. Short rows rotate independently of long ones, admitting
  the new selected story at the next available slot.
- Empty feeds, small feeds, duplicate text and USGS records never create fake
  rows. Resetting follow starts a fresh reading interval.
- Visible stories fill the four secondary slots before offscreen stories.

## Contracts
Tests inspect actual text shapes, clips and persistent state. All fixtures are
test-only; no source requests or settings writes occur.
