# dissolve.rs

## Purpose
Draws the terrain loading grid and local scene frame. Every tile's 10×10 cells
represent measured workflow progress, with a stable shuffled disappearance order.

## Components
- `draw_tile_pulse_grid`: projects each pending tile footprint and removes
  exactly floor(progress × 100) cells. Missing progress is zero; ready/no-source
  buckets are suppressed. Time only varies brightness, never completion.
- `cell_ranks`: stable per-tile permutation, avoiding random thresholds which
  can remove more/fewer cells than the reported fraction.
- `completed_cells`: clamps finite fractions and treats unknown/invalid values
  as incomplete. The last cell survives until actual completion.
- `draw_frame`: unchanged decorative viewport frame.

## Contracts
| Dependent | Expects | Breaking changes |
|---|---|---|
| Local scene | Progress and ready buckets use the same root, zoom and tile grid | Reintroducing a timed dissolve or using another body's tile size |
| Loader | Stalled work holds its exact cell set; readiness ends the indicator | Clearing a tile on disk presence alone |

## Validation
Tests check exact completion counts, monotonically disappearing cell sets,
stable tile-specific randomness, 99% versus completion, and unknown values.

An egui paint regression compares actual cell vertex positions before and after
multiple former timer cycles, then checks 75%, 99%, complete, and ready states.
