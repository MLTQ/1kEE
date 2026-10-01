# marker_style.rs

## Purpose
Share the existing event light-spire drawing between event and webcam markers,
and keep their hover/click targets tied to the actual projected geometry.

## Components
- `draw_camera_spire` paints the same green spire, small ground strike and
  selection ring in both views; scenes supply their exact projected base/tip.
- `draw_beam` draws the tapered glow and core at the supplied endpoints; scale
  changes stroke widths without changing tint or opacity.
- `local_tip` uses the local projector's screen-up vertical axis for a stable
  screen-space height. Terrain sampling and ground anchoring stay in the scene.
- `MapMarker` carries a stable id, projected base/tip, and the active size scale.
- `pick` chooses the closest hit, preferring ground anchors to nearby spires.

## Contracts
- Both hover and click consume the same scene-produced geometry.
- Only the visibly lit lower 65% of a beam is interactive; transparent tips
  cannot intercept map navigation. Small anchors retain a four-point hit floor.
- The caller supplies scaled endpoints; beam stroke widths are scaled here.
- Existing color, fade, and pulse semantics remain with the marker's renderer.

`marker_style_tests.rs` covers size-aware picking and optional GPU pixel verification.
