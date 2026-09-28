# geometry.rs

Splits connected pipeline parts at quarter-degree boundaries using segment
intersection parameters. Geographic endpoints are retained. Crossing ±180° is
unwrapped along the shorter direction, split, and wrapped independently on each
side so neither tiles nor the overview draw a line across the globe.

Consecutive segments in the same tile merge only when endpoints match. Re-entry
creates a new fragment; disconnected source parts are never joined. Iterative
Douglas–Peucker simplification is used only for the globe overview. Full local
geometry is retained at f32 coordinate precision without point-count truncation.
