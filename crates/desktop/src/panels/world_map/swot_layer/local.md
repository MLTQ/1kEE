# local.rs

## Purpose
Draw a bounded native-grid gravity window in Local Earth view.

## Contracts
A padded pixel request is sampled off-thread. Previously loaded windows stay
visible during nearby requests, using their own returned bounds. North-up edges map to the
same unwrapped geographic edges in a 48×48 sea-level drape, using the existing
local projector and view extents. Longitude wrapping happens only in the grid
reader so dateline images remain continuous around the focus. Unprojectable
cells are omitted. It does not supply geometry or marker elevations.
