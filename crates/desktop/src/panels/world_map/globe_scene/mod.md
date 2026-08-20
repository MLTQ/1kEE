# mod.rs

## Purpose

Owns the globe-mode scene composition: terrain and overlay passes, live markers,
hit-test marker output, and globe HUD rendering. It turns the immutable map
model and viewport into an egui-painted `GlobeScene` each frame.

## Components

### `GlobeScene`
- **Does**: Returns the screen-space markers needed by the parent map panel for
  click and hover interaction, plus optional beam elevation information.
- **Interacts with**: `world_map.rs`, `map_tooltips.rs`, and detail panels.

### `paint`
- **Does**: Draws globe terrain, overlays, events, cameras, live tracks,
  flights, ArcGIS features, and HUD elements in their stable layer order.
- **Interacts with**: `geography.rs`, `markers.rs`, `projection.rs`, and the
  map model.

### `GlobeLayout` / `ProjectedPoint` / `ProjectedMarker`
- **Does**: Carry per-frame camera geometry and geographic projection results
  between the scene, projection, marker helpers, and hit-test output.
- **Interacts with**: `projection.rs` and `markers.rs`.

### Surface marker projection

- **Does**: Places event, camera, and replay bases on the GPU globe's unit
  surface, then projects beam tips outward by their existing visual height.
- **Interacts with**: `projection::project_geo_unit_surface` and
  `projection::project_geo_unit_surface_elevated`.
- **Rationale**: The globe backdrop shades terrain without displacing its
  sphere, so negative synthetic terrain offsets must not pull indicators
  inside the visible globe.

### Global camera-dot layer

- **Does**: Projects every normalized registry camera on Earth while retaining
  event-to-camera links only for the selected event's 250 km nearby subset.
- **Interacts with**: `AppModel::cameras`, `show_camera_markers`, and
  `nearby_camera_snapshot`.
- **Rationale**: Directory-discovered cameras should remain visible globally
  without turning the selected-event relationship into an all-world spiderweb.

### `screen_to_latlon`
- **Does**: Maps a globe screen position back to geographic coordinates for the
  coordinate overlay.
- **Interacts with**: `world_map.rs`.

### Projection parity test
- **Does**: Compares the shared live-marker projections against the direct
  front-facing projection baseline used by the former hit-test loops.
- **Interacts with**: `project_visible_markers` and `projection::project_geo`.

### DeFlock ALPR mesh cache

- **Does**: Reuses a batched public-ALPR mesh only when both the monotonic
  source snapshot revision and every projection input are bit-identical.
- **Interacts with**: `local_terrain_scene/deflock_layer.rs` and
  `AppModel::deflock_alpr_locations`.

### Contour width routing

- **Does**: Reads the shared physical contour width once per globe frame and
  forwards it to the GPU terrain, coastline, and bathymetry contour passes.
- **Interacts with**: `geography.rs` and `AppModel::contour_stroke_width_px`.

## Contracts

| Dependent | Expects | Breaking changes |
|---|---|---|
| `world_map.rs` | `paint` returns marker IDs and positions matching the pixels drawn this frame | Changing marker order, position, or ID types |
| `markers.rs` | Projection results use the same view/layout that produced the frame | Reprojecting with different geometry or dropping front-facing state |
| `map_tooltips.rs` | Hit-test vectors contain only visible, interactive marker positions | Returning hidden or differently positioned markers |
| Globe event/camera overlays | Bases stay on the visible unit sphere and tips extend outward from those bases | Reintroducing inward terrain-radius displacement |
| Camera layer | Every visible dot and returned hit-test entry refers to the same normalized camera id | Filtering drawing and hit testing differently |
| Layer drawer | The Contours toggle gates globe terrain contours just as it gates local terrain contours | Rendering terrain contours while the toggle is off |

## Notes

- Marker draw order is part of visual fidelity: event and camera overlays are
  painted before live transport markers, and ArcGIS markers remain above them.
- Ships and flights are projected once per frame into `ProjectedMarker` lists.
  Those lists are reused for painting and interaction, preserving the exact
  source ordering and front-facing filtering previously used in both passes.
- Event and camera bases use the same unit-sphere geometry as the GPU backdrop;
  their front-face filter runs before either painter submission or hit-test
  output is created. Replay and live event beams retain the same radial tip
  height, measured outward from that surface base.
- The optional ALPR overlay is surface-projected and front-face filtered before
  mesh construction; it does not alter existing layer geometry when disabled.
- The selected physical width alters only contour uniforms, preserving cached
  geometry. Multiplier-only legacy settings resolve to their prior pixel width
  before routing, except that an old sub-pixel value is raised to 1 px.
