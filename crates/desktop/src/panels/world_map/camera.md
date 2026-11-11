# camera.rs

Canvas drag, wheel and keyboard navigation with damped momentum and cinematic
meander. `manual_input` detects navigation confined to the canvas so the parent
can cancel event follow before applying input. While event follow owns the camera,
the parent skips `apply_interaction` to avoid applying two movement controllers.
Mouse hover alone does not interrupt the idle mode. Geographic local panning
uses the renderer's current extent and oblique camera axes.

Local mode opens polar regions overhead. Drag to pan, use Shift/Ctrl-drag or
modified arrows to orbit, and press Home with the pointer over the map to return
to a north-up overhead view. Tilt runs from overhead to 75 degrees. Panning
inverts the renderer's sea-level transform. In a polar chart the controller
transports heading and momentum to the new meridian when the center moves.
