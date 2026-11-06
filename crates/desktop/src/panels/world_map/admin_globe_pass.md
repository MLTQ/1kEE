Draws a complete globe boundary generation in a single egui/wgpu callback. On each
frame it updates the contour view uniform for outlines and the spherical fill
uniform for shading. The displayed buffers therefore track the current camera,
viewport and opacity even while their replacement is being prepared.

The upload state in admin_globe_upload stages both streams using the existing
shared globe budget and publishes them together. Paint draws the displayed fill
first and then the displayed outlines using the shared contour line pipeline.
The fill pipeline is initialized on demand. Its premultiplied colors, target-format
conversion and perspective horizon match the existing Local/Globe shaders.
The frame lifecycle releases both pending and displayed geometry when the layer
is hidden, transparent, or the map enters Local.
