# map_rail.rs

## Purpose
Own the single compact map rail with independently available camera controls. Preserve
view/layer/event/camera/presentation controls alongside Settings, body selection
and Cache Blast, without operational status text taking over the map.

## Contracts
- EARTH/MOON/MARS, Cache Blast, Settings and Layers remain directly accessible.
- At narrower map widths, presentation controls move to More; with both sidebars
  open, View also contains record visibility and Focus controls.
- Body cycling retains palette changes and returns non-Earth modes to Globe.
- Meander and Follow Events are always available without a parent mode toggle.
  They are mutually exclusive; Meander exposes its speed slider when active.
- Follow Events help describes the full live pool, severity-weighted revisits and
  repeat spacing; it no longer promises a six-record tour.
- View changes stop event-follow. Replay, brief and observatory retain their actions.
- Cache Blast only drops in-memory tile caches; it does not delete disk files.
- Imported-layer state remains visible via the Layers dot. Counts remain available
  even at zero. Arbitrarily long progress messages belong in Settings → Overview.
