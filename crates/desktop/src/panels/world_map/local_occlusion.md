# local_occlusion.rs

## Purpose
Render cached, invisible contour curtains into a dedicated depth texture, which
local contour fragments consult without changing egui's UI depth state.

## Contracts
- One depth-only draw stage, at most 65,536 segments total, with a fair quota per
  resident tile. Only the exact displayed/uploaded versions participate.
- Longest depth texture edge is 2,048 pixels (at most 16 MiB). A static camera
  and unchanged source/uniforms reuse its image. Camera motion changes uniforms,
  never source geometry. Hidden/disabled terrain releases the texture.
- Depth uses yaw-rotated ground distance, monotonic along the local affine
  camera's rays. Its mapping is bounded from loaded geographic tile bounds.
- Curtains extend downward from source contours. Screen/depth tolerances retain
  crest strokes and compensate for simplification. Missing/omitted geometry
  leaves lines visible; this approximation is not a continuous terrain mesh.
- A one-pixel dummy binding supports ordinary lines/roads with occlusion off.
- CPU fallback and Moon/Mars retain their existing rendering in this first version.
