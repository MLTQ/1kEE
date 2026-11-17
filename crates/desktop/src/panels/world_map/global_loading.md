# global_loading.rs

Shows Earth Globe loading progress for enabled terrain, Atlas relief and LiDAR
coverage. Producers report in-memory metadata during scene painting; the overlay
does no source discovery, database access or geometry preparation.

The bottom-right card displays ready/requested data units and build, read, wait,
preparation, upload and unavailable counts. Its bar uses measured workflow
fractions, never elapsed time or an estimate of remaining time. Data units can
include an overview as well as tiles; the fraction is not geographic coverage.
Pending downstream preparation or source-buffer upload caps completion below
100%. Once all requested work is ready, the card disappears.

When Loading animation is enabled, active work also rotates a spinner and
pulses up to 32 pending geographic tile outlines. Eight subdivisions per edge
follow the sphere; only consecutive front-facing projections draw. Disabled
animation retains a static card/bar and lowers loading-only repaint frequency.
Unavailable-only state is static and amber. OBS and stereo scenes suppress the
overlay. Previously ready geography remains drawable throughout loading.

Frame snapshots reset before each scene. Per-slot GPU readiness stores only a
completed version token, set by actual upload callbacks and invalidated while
replacement uploads are pending. Native terrain candidates use monotonic IDs.
Continuous camera/image refinement is separate from source upload completion.

Tests cover static versus animated shapes, hidden-side outline rejection,
completion, GPU-only work, unavailable-only state, version invalidation and frame
reset. The ignored GPU preview renders the HUD with egui/wgpu and saves
`/tmp/1kee-global-loading-preview.png` for layout inspection.
