# event_tour_tests.rs

## Purpose
Test scheduling outcomes with deterministic feed fixtures and long rotations,
without network, camera animation, settings or wall-clock delays.

## Coverage
- All 40 equal-priority records get a first visit before any repeat.
- Across 4,800 visits to a balanced severity mix, higher severity gets strictly
  more visits, the most severe gets at least twice the lowest, every story gets
  coverage and every repeat has at least six intervening stops.
- A 360-record feed includes every low-priority story; each major story receives
  materially more visits than any low-priority one.
- Reordered/metadata-updated polls produce the same choices as an unrefreshed
  control over 500 visits, proving waiting credit and cooldown are retained.
- Pool sizes 2–15 keep moving without immediate repeats, and shrinking/empty
  feeds drop expired records without deadlocking recent-stop exclusion.
- Numeric severity, category fallback, recency ties, ID deduplication and real
  Factal-only filtering remain intact, including USGS records carrying a brief.

## Contracts
Tests exercise `EventTour` choices and eligibility; they do not assert internal
credit values or require random seeds.
