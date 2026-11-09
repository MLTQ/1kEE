# settings_appearance.rs

Render map palette controls in Settings → Appearance. Selection changes the map
palette immediately. Atlas help describes automatic native GEBCO/SRTM/fine-cache
refinement and explicitly preserves independent contour/other-layer controls.
Theme selection never enables/disables contours or changes other layer choices.

Atlas help now describes real 3D terrain and explains that Layer Spread affects
the surface and optional attached contours together.

`atlas_controls` exposes geographic light direction/height, optional astronomical
Sun with hour offset, relief contrast, color richness and reset. Changes affect
the current surface immediately; the existing explicit Settings save persists
them. Choosing a light never changes camera, clock, source or layer visibility.
