# replay.rs

Shared chronological playback and severity-based fading event flares. Manual
Replay uses a monotonic Instant clock with pause/resume/restart/seek controls.
Automatic continental recaps call `advance_to` with their elapsed animation
time (nondecreasing); this pins the replay clock so map flares, chyron and clock agree.

`revealed_events` exposes only the chronological prefix reached by the playhead.
Future reports never enter the recap chyron. Flare spawn times are derived from
each event's simulated timestamp so missed frames expire old flares instead of
replaying an entire missed interval at once. Input events must be sorted oldest
first; simulated range and wall duration must be positive.

`globe_scene` paints active flares using `wall_elapsed`. `replay_controls` owns
manual playback interaction. `follow_recap_tests` verifies external-clock
reveal timing, completion, and expired flares after skipped frames.

Elapsed wall time can continue past completion for the final flare fade;
simulated time, progress and the recap clock remain capped at the endpoint.
