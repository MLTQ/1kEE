# globe_terrain_reproject.wgsl

## Purpose
Keep completed full-detail contours attached to the rotating globe while the
next full-resolution image is drawn incrementally.

## Contracts
Warp contains two 80-byte ContourUniforms, a 16-byte flag vector and two 16-byte
premultiplied-linear palette colors (208 bytes total). Rays use
the same sphere radius, rotation order, perspective and physical viewport as
contour_lines.wgsl. Inverse current projection followed by saved projection
samples the retained image; uncovered pixels use the current preview. Covered
transparent pixels never mix with a second contour generation. An exact
camera match samples directly, preserving stationary pixels. Linear sampling
avoids quantizing camera motion; native vector coordinates remain unchanged.

With the palette flag enabled, red/green texture coverage is mapped to the live
major/minor colors after sampling. Direct, reprojected and preview samples all
use the same mapping, so changing themes does not redraw or retire retained detail.
