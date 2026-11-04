# follow_recap.rs

Paints the automatic recap's continent, `24hr recap` identifier, elapsed-day
clock, UTC rolling-window endpoints, reveal count and progress bar. It runs in
the Follow Events overlay before the OBS early return, so stream viewers see
the same identification as the operator. The overlay consumes no input.

Loading/framing and an empty local archive have explicit labels. Empty cached
data never claims that no incidents occurred. Text is clipped inside the panel;
it never overlaps the five-row bottom chyron or reserves sidebar width.
All timing comes from the recap's shared replay; the clock is elapsed window
time rather than UTC time of day. `world_map` owns the draw call.
