# projection.rs

## Purpose
Projects geographic points and elevations into the local oblique map. Shared
parameters keep CPU overlays aligned with the instanced contour shader while
cached CPU projection parameters avoid repeating camera setup per point.

## Components
- `LocalProjectionParams` derives geographic scale, yaw, pitch, layer spread
  and layout constants from the current camera.
- `project_logical` produces logical screen coordinates and an unclamped depth
  coordinate. Its arithmetic is shared with `local_contour_lines.wgsl`.
- `LocalProjector` applies finite/range rejection and returns screen position.
  The old clamped depth field used solely for fill triangle sorting is removed.
- `local_projection_params` supplies the GPU callback's transform.
- `project_local` is the cached CPU entry point used by map overlays.

## Contracts
| Dependent | Expects | Breaking changes |
|---|---|---|
| Local contour shader | CPU/GPU screen transforms agree | Changing projection arithmetic without updating WGSL |
| CPU overlays | Finite points within the extended viewport return a screen position | Changing clipping or coordinate units |
| Camera | Elevation exaggeration and layer spread follow the existing visual scale | Changing scale normalization |

## Validation
Existing tests compare shared parameters with shader arithmetic and the CPU
projector, including elevation, yaw and pitch. Removing fill depth does not
change screen coordinates or shader uniforms.
