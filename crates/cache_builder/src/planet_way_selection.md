# planet_way_selection.rs

## Purpose
Classify enabled way features before any coordinate preparation. Administrative
relations use their own scanner and do not require the general way pass.

## Contracts
- `has_way_layers` excludes admin and covers every way-backed feature flag.
- `classify` preserves existing tag precedence and class/name values; returns
  None when no selected feature matches, including untagged boundary members.
- `planet_all` uses the same selection for prefetch and output, so rejected ways
  never trigger compact node decompression and classification is not duplicated.
