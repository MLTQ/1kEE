# globe_terrain_texture.rs

## Purpose
Keep full-resolution terrain visible during continuous globe movement, without
redrawing tens of millions of segments in one frame.

## Components
A complete terrain image stays on its sphere via `globe_terrain_reproject.wgsl`.
Newly exposed coverage uses a bounded whole-plane preview. A second image is
refined at most two million segments per frame, using a frozen camera and source
snapshot. Movement does not restart this job: completed snapshots replace the
retained image, then another job follows the latest camera. At rest the final
image contains every native contour plane and subsequent frames draw one quad.

## Contracts
The refinement owns its uniform bind group; live camera uniform writes cannot
change an in-progress image. GPU tile versions needed by the job remain pinned.
Images from different generations never overlap within retained coverage.
Resizing, incompatible source/palette or stroke changes retire old images.
Same-camera compositing preserves exact pixels; moving reprojection interpolates
pixels without changing contour coordinates. Newly exposed areas refine over
several frames. Three viewport-sized textures are the maximum retained set.
Hiding the layer releases images, jobs, pinned sources and uploaded tiles.
