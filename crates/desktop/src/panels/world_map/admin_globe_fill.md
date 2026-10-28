# admin_globe_fill.wgsl

Projects subdivided unit-sphere triangles through the globe's yaw/pitch camera.
Perspective-correct sphere coordinates allow the fragment shader to discard the
hidden surface at the true perspective horizon `normal.z = 1 / camera_distance`.
This avoids back-side regions showing through near the limb. No terrain elevation
is applied; the globe backdrop itself is a unit sphere. The 64-byte uniform and
52-byte triangle instance layouts must stay aligned with admin_globe_pass.rs.

Color output follows egui: gamma-encoded premultiplied color for unorm targets,
converted RGB for sRGB targets, with linear alpha in both cases. This keeps the
same shade-opacity control visually consistent across the two map views.
