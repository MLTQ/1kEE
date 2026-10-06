# contour_lifecycle.rs

## Purpose
Release globe line geometry when layers are hidden or the map enters local
view. Previously CPU instance sets and GPU layer buffers survived indefinitely.

## Contracts
The canvas calls `begin_frame` before scene drawing and `end_frame` afterward.
Requested CPU builds remain active even before they can draw. Inactive entries
drop source/instance Arcs, retain occupied worker slots and discard late results.
GPU cleanup runs after all prepares and keeps exactly the layers drawn this frame.
No shader or uniform layout changes. Tests verify actual Arc release and late
worker rejection; the hardware lifecycle regression verifies GPU buffer release.

Lifecycle also brackets the persistent Earth tile cache and clears its GPU
staging/display generations when not prepared. Worker slots remain occupied
until completion so hiding the map cannot create parallel stale workers.

The first prepare resets the shared globe upload budget before any layer uploads.
