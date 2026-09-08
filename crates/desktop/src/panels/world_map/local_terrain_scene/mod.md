# mod.rs

## Purpose
Owns the high-zoom local terrain scene: camera layout, contour/overlay composition, terrain projection, and the scene-level tests that validate the local map stack against real contour cache data. It is the handoff target when the world map leaves the globe view.

## Components

### `render_local_terrain_scene`
- **Does**: Drives the local terrain renderer, assembles terrain/road/water/uploaded-layer overlays, and coordinates the split helper modules in this folder
- **Interacts with**: `contour_asset.rs`, `terrain_field.rs`, `road_layer.rs`, `water_layer.rs`, `projection.rs`, `ui_overlays.rs`

### Helper submodules (`projection`, `geography`, `markers`, `deflock_layer`, `dissolve`, `ui_overlays`)
- **Does**: Split projection math, geographic drawing, marker rendering, transition effects, and HUD overlays out of the main scene entrypoint
- **Interacts with**: `render_local_terrain_scene` and world-map state in `AppModel`

### DeFlock ALPR overlay

- **Does**: Projects the optional public ALPR snapshot into the same local
  terrain space as camera markers, then reuses its batched mesh whenever the
  snapshot revision and local projection inputs are unchanged.
- **Interacts with**: `deflock_layer.rs` and
  `AppModel::deflock_alpr_locations`.

### Local scene tests
- **Does**: Exercise the contour-loading/projection path against cached focus data so the local terrain stack keeps a working end-to-end sanity check
- **Interacts with**: `contour_asset::load_srtm_region_for_view`, egui layout helpers

### Contour width routing

- **Does**: Converts the shared physical primary width to the existing local
  relative scale before threading it through ordinary and transition contour
  stacks. This retains alpha and major/minor weight relationships.
- **Interacts with**: `AppModel::contour_stroke_scale_for_pixels_per_point`
  and `geography.rs`.

### Local stroke floor

- **Does**: Raises each derived local contour, coastline, and bathymetry stroke
  to one physical pixel when the selected primary width or transition fade
  would otherwise make it sub-pixel.
- **Interacts with**: `draw_contour_stack` and `geography.rs`.
- **Rationale**: The width control's lower bound must be visible in both map
  views without flattening local stroke relationships at normal widths.

### Contour projection budget

- **Does**: Projects ordered local contours in fixed Rayon chunks and stops at
  the existing render-point safety cap before allocating paths that would be
  discarded.
- **Interacts with**: `draw_contour_stack` and `contour_asset.rs` merged arcs.
- **Rationale**: Keeps accepted contour ordering and visual output identical
  while preventing a dense tile arrival from projecting the entire 13×13
  source set after the cap is already exhausted.

### Local contour envelope

- **Does**: Requests, builds, and merges a 13×13 local source grid. This keeps
  overlapping outer-tile contours available at the viewport edge without
  expanding the globe-mode request envelope; existing AABB culling still
  limits projection to the local scene.
- **Interacts with**: `contour_asset.rs` and `srtm_focus_cache` region APIs.

### Elevation-fill worker scheduling

- **Does**: Retains one mesh build and its sampled 61×61 elevation surface
  while the camera moves, then builds the newest key after that worker
  completes instead of cloning contours and spawning a replacement worker
  every frame. Disconnected/panicked workers clear their marker so the next
  frame retries.
- **Interacts with**: `draw_elevation_fill`, `build_elev_fill_mesh`, and the
  local repaint loop. The retained surface supplies beam and marker anchors.

### Surface-aligned indicators

- **Does**: Samples the exact triangles used by the displayed elevation-fill
  mesh before projecting event/camera markers and the targeting beam. Marker
  glyphs keep a small clearance above that ground contact.
- **Interacts with**: `markers.rs`, `draw_local_beam`, and `ElevationSurface`.
- **Rationale**: Raw SRTM samples and the interpolated visible mesh can differ;
  using the latter prevents indicators from appearing above or below topology.

### Viewport camera dots

- **Does**: Pre-culls and projects every normalized Earth camera inside the
  local viewport, while drawing event link lines only to the 250 km nearby
  subset.
- **Interacts with**: `AppModel::cameras`, `show_camera_markers`, and the local
  projection/elevation surface.

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `world_map.rs` | This module remains the local-mode renderer entrypoint and can be switched to by zoom/focus state alone | Moving the local scene entrypoint or changing its model/context contract |
| Overlay renderers | Imported user layers, roads, water, and contours share the same local projection space | Changing coordinate transforms without updating overlay helpers |
| Scene tests | The local contour loader stays reachable through `contour_asset::load_srtm_region_for_view` for end-to-end validation | Renaming/removing that loader without updating the tests |
| Globe scene | A shared physical primary width produces matching derived local widths, with every local stroke clamped to at least 1 px | Applying a different conversion or bypassing the floor in transition rendering |
| Cinematic movement | At most one elevation-fill mesh worker is active while camera-derived keys change | Replacing the receiver and spawning a worker per frame |
| Local markers and beam | Ground contacts match the exact elevation surface of the mesh painted this frame | Sampling a separate terrain source or pairing a stale mesh with a newer grid |

## Notes
- The scene-level tests are intentionally tolerant of missing local cache data: they return early when the shared focus cache is unavailable instead of making the suite depend on large fixture assets.
- The optional ALPR overlay is Earth-only and defaults off, so it cannot alter
  lunar, Martian, or existing local-terrain output unless explicitly enabled.
- The local geography helpers receive the same scale for coastline and
  bathymetry linework, while unrelated roads, boundaries, and marker strokes
  deliberately retain their independent visual contracts.
- Elevation-fill workers are named to make any future background failure
  diagnosable from the native crash report.
- If a mesh worker exits without sending a result, the stale mesh stays visible
  and the single-flight gate is released rather than leaving elevation fill
  permanently blocked.
- Elevation-fill cache keys include the active body. This prevents a retained
  Moon/Mars surface from being reused after a body change, and Earth-only event
  and camera data is not projected into planetary local scenes.
- The local source and build radii intentionally match at 13×13: an uncached
  Moon/Mars tile outside a smaller central window can uniquely own a contour
  that crosses the visible edge. The loader returns its ready/progress snapshot
  to the pulse grid and overlay, avoiding extra cache queries during local
  paint.
- The elevation-fill worker takes only the local viewport's padded contour
  subset; the wider source envelope remains available to line rendering but
  cannot inflate fill-worker cloning or bias local elevation interpolation.
- Local contour line projection is chunked in source/elevation order. Rayon
  still handles point projection, while egui submission stays serial and
  stops before work the existing point budget would never draw.

### 3DEP deep zoom

- **Does**: `local_render_zoom` no longer clamps the tile spec at 20. The 3DEP
  tiers continue the ladder to `LOCAL_ZOOM_MAX`, so tile detail keeps improving
  across the whole visual zoom range instead of freezing two thirds of the way
  down it.
- **Interacts with**: `srtm_focus_cache::zoom`, `hillshade_layer`.
- **Rationale**: The visual scale already reached ~0.6 km across while contours
  were still being derived at ~40 m/px from 30 m SRTM. Everything below the old
  clamp was interpolation.
- Earth contour requests use `prefetch_radius_for_zoom`, which tightens the
  envelope for the network-sourced tiers. Moon and Mars keep the full radius —
  they have their own spec ladders and local sources.
