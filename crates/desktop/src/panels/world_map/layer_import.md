# layer_import.rs

## Purpose
Schedules automatic focused road/water coverage for visible local map layers.

## Contracts
- Checks are limited to twice a second and respect active ingest work.
- Road requests for focus and viewport are batched into `road_import`'s worker;
  completion is polled even if roads have since been hidden. No road database or
  inventory work occurs synchronously in the render loop.
- Radius, focus selection, job deduplication and UI log wording remain stable.
- Water uses the same worker contract with an independent single-flight gate.
  Focus and viewport checks are batched so neither can starve the other.
