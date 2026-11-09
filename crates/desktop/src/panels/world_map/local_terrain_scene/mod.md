# mod.rs

The independent SWOT gravity overlay draws on the Earth sea-level plane before
contours. It requests a padded, geographically aligned native-grid window via
`swot_layer`; it does not change elevations, occlusion or terrain streaming.

## Purpose
Owns the high-zoom local terrain scene: camera layout, contour/overlay composition, terrain projection, and the scene-level tests that validate the local map stack against real contour cache data. It is the handoff target when the world map leaves the globe view.

## Components

### Explicit Earth Local view
- `is_active` honors the Local toggle independently of selected events/cities or
  a raw SRTM directory. Cached contours, streamed terrain and vectors remain usable.
- `local_focus` uses the viewport center when no event/city is selected, so Local
  works during ordinary map navigation as well as event inspection.

### OBS view
- Skips decorative framing, navigation legends, tile-loading grids and progress
  cards during clean window capture. Terrain loading and all configured scene
  layers continue; this flag does not change camera motion or selection.
- Enabled event pips remain visible in OBS view even during meander.

### `render_local_terrain_scene`
- **Does**: Drives the local terrain renderer, assembles terrain/road/water/uploaded-layer overlays, and coordinates the split helper modules in this folder
- **Interacts with**: `contour_asset.rs`, `terrain_field.rs`, `road_layer.rs`, `water_layer.rs`, `projection.rs`, `ui_overlays.rs`

### Helper submodules (`projection`, `geography`, `markers`, `deflock_layer`, `dissolve`, `ui_overlays`)
- **Does**: Split projection math, geographic drawing, marker rendering, transition effects, and HUD overlays out of the main scene entrypoint
- **Interacts with**: `render_local_terrain_scene` and world-map state in `AppModel`
- Local GPU callbacks receive the egui context for upload continuation and a
  shared pass clock, including frames when only road overlays are visible.
- Earth roads are submitted independently of contour availability; terrain
  loading cannot suppress an already-ready road overlay.

### DeFlock ALPR overlay

- **Does**: Projects the optional public ALPR snapshot into the same local
  terrain space as camera markers, then reuses its batched mesh whenever the
  snapshot revision and local projection inputs are unchanged.
- **Interacts with**: `deflock_layer.rs` and
  `AppModel::deflock_alpr_locations`.

### Local scene tests
- **Does**: Exercise the contour-loading/projection path against cached focus data so the local terrain stack keeps a working end-to-end sanity check
- **Interacts with**: `contour_asset::load_srtm_region_for_view`, egui layout helpers
- **Rationale**: Tests choose an explicit geographic focus rather than depending
  on runtime-seeded event data.
- `contour_seam_tests.rs` now checks actual complete-core reads and matching
  boundary endpoints, replacing the old numerical deep-tier budget assertion.

### Contour width routing

- **Does**: Converts the shared physical primary width to the existing local
  relative scale before threading it through native contour
  passes. This retains alpha and major/minor weight relationships.
- **Interacts with**: `AppModel::contour_stroke_scale_for_pixels_per_point`
  and `geography.rs`.

### Local stroke floor

- **Does**: Raises each derived local contour, coastline, and bathymetry stroke
  to one physical pixel when the selected primary width or pass fade
  would otherwise make it sub-pixel.
- **Interacts with**: `contours.rs` and `geography.rs`.
- **Rationale**: The width control's lower bound must be visible in both map
  views without flattening local stroke relationships at normal widths.

### Native contour rendering

- Earth, Moon and Mars all use `contours.rs` and the native instanced renderer.
- Body-specific classification keeps 50 m Earth majors and 1,000 m lunar/Martian
  majors, including negative elevations. Palette and camera changes use uniforms.
- The compositor clips overlapping source geometry into disjoint owned cores.
- Cold starts show Preparing terrain while workers/uploads complete; later
  updates retain prior GPU terrain. There is no CPU contour projection fallback
  or per-frame point cap. CPU projection remains for independent overlays.

### Local contour envelope

- **Does**: Selects enough nonoverlapping Earth cores to cover the oblique
  viewport. Moon/Mars retain their source envelope; composition clips the
  overlaps to owned geographic cores before GPU instance building.
- **Interacts with**: `contour_asset.rs` and `srtm_focus_cache` region APIs.

### Terrain composition and indicator heights

- Contours draw once. The legacy interpolated fill, its background worker,
  surface cache, color ramps and duplicate background contour pass are removed.
- Event and webcam anchors use the nonblocking Earth elevation cache with the
  existing clearance. The targeting beam retains its nearby-contour estimate.
- The selected USGS or Washington DNR hillshade uses its existing sea-level projection;
  it does not provide a terrain surface or occlude contours.
  Drawing and requesting both stop while detailed terrain streaming is disabled.

### Viewport camera dots

- **Does**: Pre-culls and projects every normalized Earth camera inside the
  local viewport, while drawing event link lines only to the 250 km nearby
  subset.
- **Interacts with**: `AppModel::cameras`, `show_camera_markers`, and the local
  projection and nonblocking elevation samples.

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `world_map.rs` | This module remains the local-mode renderer entrypoint and can be switched to by zoom/focus state alone | Moving the local scene entrypoint or changing its model/context contract |
| Overlay renderers | Imported user layers, roads, water, and contours share the same local projection space | Changing coordinate transforms without updating overlay helpers |
| Scene tests | The local contour loader stays reachable through `contour_asset::load_srtm_region_for_view` for end-to-end validation | Renaming/removing that loader without updating the tests |
| Globe scene | A shared physical primary width produces matching derived local widths, with every local stroke clamped to at least 1 px | Applying a different conversion or bypassing the floor in native rendering |
| Local indicators | Marker sampling never blocks paint; the targeting beam uses loaded contours | Adding synchronous elevation I/O during rendering |

## Notes
- The scene-level tests are intentionally tolerant of missing local cache data: they return early when the shared focus cache is unavailable instead of making the suite depend on large fixture assets.
- The optional ALPR overlay is Earth-only and defaults off, so it cannot alter
  lunar, Martian, or existing local-terrain output unless explicitly enabled.
- The local geography helpers receive the same scale for coastline and
  bathymetry linework, while unrelated roads, boundaries, and marker strokes
  deliberately retain their independent visual contracts.
- The local source and build radii intentionally match at 13×13: an uncached
  Moon/Mars tile outside a smaller central window can uniquely own a contour
  that crosses the visible edge. The loader returns its ready/progress snapshot
  to the pulse grid and overlay, avoiding extra cache queries during local
  paint.
- Native contour instances build on workers and upload in bounded pieces.
  No terrain paths are projected or tessellated on the UI thread.

### 3DEP deep zoom

- Earth requests now pass through `contour_fallback.rs`. If fine tiles do not
  cover the viewport, SRTM stays visible at the full requested camera zoom.
  The Earth loading grid uses the returned source zoom/radius, while projection
  and unrelated overlays continue using the camera zoom.

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

### GPU contour rendering

- **Does**: `contours::batches` picks up per-tile instance batches from
  `local_contour_pass`, and `contours::paint` submits one paint callback
  per contour pass for all three bodies. Upload-only staging never draws; the
  visible pass uses only the published GPU display.
- **Interacts with**: `local_contour_pass`, `projection::local_projection_params`.
- **Rationale**: The removed CPU stack projected and tessellated every point every frame,
  which is what forced `MAX_CONTOUR_RENDER_POINTS`. Geometry uploaded once per
  source tile is redrawn for free, so no body is subject to a per-frame
  point budget at all — the ceiling became a resident-memory budget instead.
- The old CPU contour stack, point-budget selection and unused transition
  overlay are removed. Full visible geometry is limited by residency, not a
  per-frame projection budget.
- Body-specific classification keys preserve 50 m Earth majors and 1,000 m
  lunar/Martian majors. Colors remain live uniforms; body switches cannot reuse
  another body's classification, including when an Arc/tile ID is shared.

- The loading grid uses per-tile measured workflow fractions from
  `contour_loading.rs` rather than a repeating timer. Mars supplies its own
  half-extent, matching the tile identifiers in its progress snapshot.
  Completed source/build stages, decoded rows, and accepted merge publication
  govern the grid; brightness alone may breathe while work waits.

- Earth selection now follows disjoint core footprints and actual oblique view
  size, with one spare hosted ring. The coverage test includes worst-case focus
  displacement within a core. Moon/Mars keep their existing envelope.

### Local Flock inventory rendering

The existing batched public-position mesh also accepts the independent Flock
TSV snapshot through `AppModel::public_camera_positions`. Its cache key includes
both source revisions, both visibility flags, and the inactive-record filter.
Filtering or reloading cannot reuse stale geometry; Earth-only projection and
culling stay unchanged. No point-count drawing limit truncates the inventory.

Public fuel routes load quarter-degree archive tiles on workers and draw regardless of available contours. OSM remains an independent optional local pipeline layer. Geographic projection keys include focus, oblique view, extents, layout and clip rectangle.

Offshore platforms share the public-route projection inputs at sea level and draw independently of contour availability. Cached meshes and hover use the same projection; their toggle/filter state is separate from pipelines.

Local cable hover reuses the exact projected route vertices during painting. Only the explicitly supplied built-in catalogue supplies metadata, and hidden layers do not offer candidates.

Event indicators follow the Events layer toggle on Earth, independently of Meander, Follow Events or Focus capture. Frame chrome is hidden only by Focus capture.

Event markers use `map_events` so a followed target stays visible after aging out of the live page.

### Visible tile residency

Before loading, the scene supplies the exact projection and clip rectangle to
`contour_residency`. Decoded tiles outside it are unloaded, with an eight-point
edge guard and elevation-aware bounds. Both Earth source tiers are pruned;
Moon/Mars preserve their ownership envelope for tiles that can cross the view.
GPU instance residency follows the displayed Earth tiles, and hidden contours
release their instance copies. Disk cache contents are unchanged.

Webcam/event spires share saved marker sizing. Bases use nonblocking Earth
elevation samples; tips extend along the projector’s screen-up vertical axis at a stable
logical-point height. `MapMarker` returns the exact drawn base/tip/scale for hover
and click picking, independent of terrain availability. Links use ground bases.

### Bounded LOD and display handoff

`lod.rs` separates source selection from camera projection. It prefers 81 tiles,
allows 121 to prevent threshold oscillation, and uses at most 225 at the widest
view where the coarsest existing source cannot cover the screen with fewer.
Event-follow arrival at zoom 9.5 requests 81 instead of 529 tiles. The same
policy applies to all three bodies without changing persisted address grids.

`handoff.rs` prepares a background composition of ready source cores before GPU
staging. Missing edge cells retain coarse pieces only in their own gaps; they
cannot block the rest of a detail upgrade. Candidate batches upload without
drawing overlapping tiers, while the prior picture remains visible. All bodies
share clipped geometry and publish only after their native GPU uploads finish. Progress describes
actual loading. Offscreen tile references are pruned by owned cores
and elevation bounds. Root/body changes, exit, and cache reset clear the handoff.
See `composition` and `handoff` for ownership, coalescing, and publication.

Camera proximity links draw only with the separate Nearby camera links toggle
enabled, matching globe mode. Webcam spires remain independently visible.

The local Earth GPU contour pass applies `AppModel::contour_occlusion`.
Occluders use the exact displayed tile versions through the existing staged
handoff. All three bodies use the same optional visibility pass after their GPU upload.
Earth GPU batches cache only geometry/classification; the visible pass supplies
the current palette and minor-contour weighting each frame. Theme changes reuse
the displayed batches, uploads, LOD handoff and occluders without waiting for tiles.

Passes the same capacity-color flag to public pipelines and OSM pipelines. Public routes use the shared gas/oil capacity bands; OSM has no retained quantity and uses the unavailable color.

Administrative boundaries delegate to `admin_render`: selected levels, optional
complete-region shading and safe clipping. The overlay remains independent of
contour availability and paints before event/camera markers.

Selected NCEI gravity-anomaly imagery uses the same sea-level raster drape as
SWOT, below contours/markers and independent of terrain streaming. It reads
only its own prepared regional product, never altering elevations.

Local Earth projects buildout campuses and geolocated rules at ground level and passes visible marker positions to the shared picker.

### Atlas 3D surface
Atlas now uses measured-height GPU terrain rather than a sea-level image quad.
The surface copies the shared local projector, including vertical exaggeration
and layer spread, and derives optional contours on that same opaque geometry.
It does not upload or draw the old detached vector contour stack in Atlas.
Source terrain acquisition and other overlays keep their existing paths. The old
vector-upload progress/grid is suppressed; Atlas reports its own source/staging
state. Independent OSM extraction progress remains visible. Other bodies/themes
preserve the original contour rendering contract.
