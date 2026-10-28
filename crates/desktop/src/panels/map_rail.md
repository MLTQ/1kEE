# map_rail.rs

## Purpose
Own the single compact map rail shared by normal and cinematic views. Preserve
view/layer/event/camera/presentation controls alongside Settings, body selection
and Cache Blast, without operational status text taking over the map.

## Contracts
- EARTH/MOON/MARS, Cache Blast, Settings and Layers remain directly accessible.
- At narrower map widths, presentation controls move to More; with both sidebars
  open, View also contains record visibility and cinematic/Focus controls.
- Body cycling retains palette changes and returns non-Earth modes to Globe.
- View changes stop event-follow; cinematic off stops meander/follow. Replay,
  follow, brief, observatory and speed controls preserve their original actions.
- Cache Blast only drops in-memory tile caches; it does not delete disk files.
- Imported-layer state remains visible via the Layers dot. Counts remain available
  even at zero. Arbitrarily long progress messages belong in Settings → Overview.
