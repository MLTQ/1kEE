# atlas_forest_tests.rs

Pixel-area fixtures pin concave bays, idempotent duplicated source cells, invalid
and open ring rejection, date-line unwrapping and polygons enclosing the canvas
with every original vertex outside it. Geometry/occlusion/light behavior is
verified separately on the actual GPU surface with a synthetic forest mask.

Source-cell upper limits are the immediate previous float, avoiding duplicate neighbor reads without dropping a narrow strip near a boundary.
