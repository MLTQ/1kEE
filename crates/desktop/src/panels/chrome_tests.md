# chrome_tests.rs

Integration tests render real chrome at wide, minimum-window and narrow-map
widths. Assert one row, no overlapping labels and access to the required primary
controls. Pointer-driven navigation verifies Settings in cinematic mode, embedded
Terrain Library, Activity, Focus hide/restore, body cycling and import in Layers.
Synthetic records exercise realistic badge widths without network or save actions.

Every settings section must leave Save Settings fully inside its clip rectangle
and the screen after several layout frames, including the minimum 720-point
window height.

The fixture uses a quarter-pixel-aligned contour width to prevent the existing
Layers slider from rounding a legacy default and persisting during headless render.
