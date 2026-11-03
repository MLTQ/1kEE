# raster_mesh_tests.rs

## Purpose
Catch regional-image disappearance at deep local zoom and dateline placement.

## Contracts
A whole-U.S. raster must still cover all canvas corners at close zoom for
rotated and steeply pitched local views. UVs stay inside the original image.
Alaska's unwrapped source is visible from either longitude representation;
far-away regional images produce no local geometry. Existing SWOT tests pin
shared globe geometry and native-window UVs.
