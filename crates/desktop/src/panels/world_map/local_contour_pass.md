# local_contour_pass.rs

## Purpose

Draws the local terrain scene's contours as GPU-instanced line segments. It is
the tangent-plane sibling of `contour_pass.rs`, and exists because 3DEP put two
orders of magnitude more geometry in front of the local scene than SRTM ever
did — a measured bucket-10 tile holds ~9 800 contours and 4.3 M points — which
the CPU projection and egui tessellation could not keep up with.

## Components

### `instances_for_tile`

- **Does**: Returns a tile's segment instances, building them on a background
  thread the first time they are asked for. Returns `None` until the build
  lands, so the scene draws whichever tiles are ready.
- **Interacts with**: `contour_asset::LocalContourLoad::tiles`, Rayon.
- **Rationale**: Geometry is keyed **per source tile**, not per merged set.
  Each tile's `Arc<Vec<ContourPath>>` is stable once loaded, so its instances
  are built once, uploaded once, and never touched again. A tile arriving
  mid-navigation allocates its own buffer and disturbs nothing.

### `LocalSegmentInstance`

- **Does**: One polyline segment: two `(lon, lat, elevation_m)` endpoints, a
  baked premultiplied-linear colour, and a major/minor flag. 32 bytes.
- **Rationale**: Endpoints are raw geography and the shader applies the whole
  transform, so a tile's instances stay valid under any camera motion — panning,
  rotating and zooming never rebuild or re-upload anything.

### `LocalContourCallback`

- **Does**: Carries one pass's uniforms and the frame's tile batches. `prepare`
  writes the uniform slot and uploads new tiles; `paint` issues one instanced
  draw per resident tile.
- **Interacts with**: `local_contour_lines.wgsl`, `LocalContourPassResources`.

### `LocalContourPassResources::enforce_budget`

- **Does**: Evicts least-recently-drawn tiles until resident geometry fits
  `settings_store::local_contour_vram_budget_bytes`. Tiles drawn this frame are
  never evicted.
- **Rationale**: The CPU path had to cap *points* because each one cost
  projection and tessellation every frame. Here they cost only memory, and
  drawing is flat in their number, so the ceiling is "how much geometry stays
  resident" rather than "how much can be redrawn in 16 ms".

## Contracts

| Dependent | Expects | Breaking changes |
|---|---|---|
| `local_terrain_scene` | Submitting a pass never blocks on geometry building | Building instances inline in `new` or `prepare` |
| `local_contour_lines.wgsl` | `LocalContourUniforms` field order and size are unchanged | Reordering or resizing without matching the WGSL |
| Scene stroke controls | Width and fade land only in uniforms | Baking either into instances, which would force a rebuild per frame |
| `contour_asset::clear_caches` | `clear_instances` drops every cached build | Retaining instances past a cache reset |

## Notes

- Earth only. Lunar and Mars merges apply exclusive midpoint ownership to
  de-overlap their tiles, which the per-tile path would have to reproduce; they
  keep the CPU renderer, where geometry volume is not a problem.
- The scene falls back to the CPU stack whenever no batch is ready yet, so the
  handover is invisible rather than a blank frame.
- Major and minor contours carry different stroke widths, and the CPU width
  formula ends in a `max()` floor. Both widths are therefore computed host-side
  and selected per instance, rather than derived from one width by a multiplier
  that the floor would break.
- **Widths crossing this boundary are physical pixels**, matching
  `contour_pass`, which is handed `AppModel::contour_stroke_width_px` directly.
  The scene's own stroke helpers return logical *points* because they feed
  `egui::Stroke`, so `gpu_contour_stroke_width_px` scales by
  `pixels_per_point` on the way in. Passing points straight through drew a
  half-pixel core inside a full-width feather, which read as soft, too-heavy
  lines at the 1 px setting. `the_minimum_width_setting_is_one_physical_pixel`
  pins it.
- The anti-alias fringe is `contour_pass::contour_feather_px`, shared rather
  than reimplemented, so equal widths render at equal weight in both views.
- `version_key` folds the tile's `Arc` pointer with the palette. Contours are
  immutable once loaded, so pointer identity is a sound cache key, and a theme
  change invalidates the baked colours.
- Uploads are capped per frame so a jump into a fully-cached area spreads its
  envelope over consecutive frames instead of stalling on one.
- A single tile can exceed the device's `max_buffer_size` on its own — a dense
  bucket-10 tile is ~4.3 M segments — so uploads go through
  `contour_pass::split_instance_buffers` rather than one allocation per tile.
- Two tests guard the CPU/GPU boundary: `the_shader_declares_the_same_uniform_fields_in_the_same_order`
  parses the WGSL and compares it to the Rust struct, and
  `local_projection_matches_the_shader` in `local_terrain_scene/projection.rs`
  pins the arithmetic the vertex shader was transcribed from. wgpu validates the
  vertex attribute layout against the shader at pipeline creation, so that class
  of mismatch fails loudly at startup.
