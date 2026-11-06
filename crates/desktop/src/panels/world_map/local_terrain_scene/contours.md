# contours.rs

## Purpose
Build and draw the full local contour geometry through the native GPU renderer
for Earth, Moon and Mars. The scene never projects/tessellates terrain on the UI
thread, including during a cold start.

## Contracts
- Earth majors remain at 50 m; Moon/Mars majors remain at 1,000 m, including
  negative elevations. Body-specific style keys prevent cross-body cache reuse.
- Inputs are the compositor's disjoint geographic tile pieces, preserving
  ownership at overlapping lunar/Martian source edges and during LOD changes.
- Workers build instances once. Camera, palette, fade and stroke changes use
  live uniforms; no per-frame point limit drops native contours.
- `paint` reuses the exact scene/residency projection and physical stroke widths;
  camera math is not recomputed separately for the visible pass.
- Optional contour occlusion works in the same projected geometry on all bodies.
- First loads wait for GPU uploads; later handoffs retain prior GPU terrain.
