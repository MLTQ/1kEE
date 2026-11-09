# tree_layer_tests.rs

Regression fixtures verify concave forest area/bays, preserved baked heights,
interpolated heights at tessellation intersections, open/invalid ring handling,
short date-line edges and split outlines at missing projections. A real egui
stroke tessellation of a nearly reversing hairpin must stay inside the source
bounds plus two pixels: closed-path corner spikes cannot escape the footprint.
The opt-in runtime child renders existing forest data over real Atlas terrain.
