# map_rail.rs

Local Earth also exposes **TARGET**, a radial inspection selection tool. It is
absent on Globe and other celestial bodies. Activating it stops camera motion
and event following, and leaves the Local camera position and viewing angles
in place. Clicking the selection enters the independent free-flight Target
viewport; Escape cancels or returns to Local. While selecting, a logarithmic
radius slider spans 0.1–100 km and the wheel can zoom Local to frame the circle.
Teal catalog footprints guide placement even when the Lidar layer is disabled.

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
- Local snapshots the visible globe center before cancelling a recap, so restoring
  the recap's saved camera cannot redirect the requested Local area.
- Cache Blast only drops in-memory tile caches; it does not delete disk files.
- Imported-layer state remains visible via the Layers dot. Counts remain available
  even at zero. Arbitrarily long progress messages belong in Settings → Overview.

Follow help describes the five-minute live / one-minute regional recap cycle, including the Middle East.
Meander cancels through `stop_event_follow`, including any active recap, so no
automatic playback survives switching idle modes.
