# camera.rs

Canvas drag, wheel and keyboard navigation with damped momentum and cinematic
meander. `manual_input` detects navigation confined to the canvas so the parent
can cancel event follow before applying input. While event follow owns the camera,
the parent skips `apply_interaction` to avoid applying two movement controllers.
Mouse hover alone does not interrupt the idle mode. Geographic local panning
uses the renderer's current extent and oblique camera axes.
