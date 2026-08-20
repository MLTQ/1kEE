# markers.rs

## Purpose

Contains the pixel-level egui drawing primitives for live vessels, flights,
events, cameras, and their links on the globe. It intentionally separates
marker styling from scene composition and geographic projection.

## Components

### `draw_ships`
- **Does**: Paints AIS vessel halos, headings, and selection rings.
- **Interacts with**: `globe_scene/mod.rs` for source tracks and precomputed
  visible screen positions.

### `draw_flights`
- **Does**: Paints ADS-B flight halos, heading glyphs, and selection rings with
  theme-aware category colors.
- **Interacts with**: `globe_scene/mod.rs` precomputed screen positions and
  `FlightTrack::category`.

### Event and camera marker helpers
- **Does**: Paints event beams/flares, green camera pips, and links between
  selected events and cameras.
- **Interacts with**: `globe_scene/mod.rs` and `ProjectedPoint`.

## Contracts

| Dependent | Expects | Breaking changes |
|---|---|---|
| `globe_scene/mod.rs` | Each live marker is drawn once in source order with its exact projected position | Reprojecting here, changing order, or changing glyph geometry |
| `world_map.rs` | The visual marker center agrees with the scene hit-test position | Offsetting marker centers without updating hit testing |
| Themes | Flight categories retain their existing theme color mapping | Changing category/color selection |

## Notes

- Visibility and geographic projection belong to the scene module so a single
  projected list can feed drawing, click handling, and hover handling.
- Draw helpers must not filter or reorder their input; that would diverge their
  output from `GlobeScene` hit-test vectors.
