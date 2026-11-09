# atlas_runtime_tests.rs

## Purpose
Catch the gap between a manually loaded native crop and the running application.

## Coverage
- A missing original dataset reaches the canvas notice state asynchronously,
  rather than failing silently behind the coarse overview.
- The ignored hardware regression calls production `globe_scene::paint` at
  the user's 28.67 zoom, allowing real viewport selection, tool discovery,
  source loading, publication, texture upload and rendering to complete.
- It compares refined pixels against the coarse frame, verifies native-scale
  texel density and preserves the operator's contour choice.
- Run the compiled test under `/usr/bin:/bin:/usr/sbin:/sbin` PATH to reproduce
  a Finder launch; `ONEKEE_ATLAS_ROOT` chooses existing read-only terrain and
  `ONEKEE_ATLAS_RUNTIME_PREVIEW` optionally saves the actual scene render.

No app window is opened and no user settings or original sources are modified.

The globe regression now waits for actual GPU height/color staging, not only CPU
image publication. The Local production-controller regression loads its real
viewport, draws a solid elevation surface, then enables optional surface contours
without changing terrain geometry. It waits an extra publication frame before
comparison so removing a loading notice cannot satisfy the contour assertion.
Separate screenshots can capture both states.
