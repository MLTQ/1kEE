# infrastructure_hover.rs

Collects visible pipeline/platform hits during the current scene and paints one
noninteractive metadata card afterward. `begin` receives the canvas response's
hover position; off-canvas/covered/dragging pointers never start a hit query.

- Deduplicates repeated tile/segment hits by source ID and feature kind, keeping
  minimum distance. Platform markers take priority over pipeline lines.
- The closest record gets full metadata; up to four overlapping records are
  named with their source/ID, plus a total count. Sources stay independent.
- Preserves operator versus owner, unknown status, route accuracy and static
  platform location caveats. OSM displays only metadata retained in its cache.
- Coordinates are in logical pixels; both local and globe picking use a 7px
  tolerance. Screen-constrained card uses wrapping to stay readable near edges.
- No network/disk lookup on hover. Clear at every frame so hidden or replaced
  layers cannot leave stale cards. Existing marker cards are suppressed when
  infrastructure owns the tooltip, preventing multiple stacked cards.

Headless egui regression verifies actual card text (operator, unknown status, depth, attribution, static-position caveat and overlap summary), including a near-edge pointer.
