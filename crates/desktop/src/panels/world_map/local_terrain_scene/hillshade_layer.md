# hillshade_layer.rs

## Purpose

Projects selected USGS or Washington DNR hillshade onto the local
sea-level plane. The image service supplies relief shading from its own source
elevation data; this independent overlay creates no elevation surface and does
not occlude contours.

## Components

### `draw_hillshade`

- **Does**: Builds a 48×48 drape mesh over the cached texture's bounds,
  projecting each vertex through `project_local` at sea level, and
  paints it as a single textured mesh.
- **Interacts with**: `projection::project_local`,
  `local_terrain_scene::render_local_terrain_scene`.
- **Rationale**: Retains the projection previously used with terrain fill off.
  The image conveys relief but is not draped over contour elevations.
  Unprojectable vertices drop only the cells that touch them.

### `ensure_requested` / `upload_pending`

- **Does**: Schedules one background fetch when the view leaves the cached
  bounds, then uploads the decoded image to a texture on the UI thread.
- **Interacts with**: `hillshade_source::Source::fetch`.
- **Rationale**: Texture upload needs the egui context, so the fetch thread can
  only hand back a decoded `ColorImage`.
- Source identity travels with requests, pending images and textures; changing
  products cannot display the previous product. One worker owns the slot even
  across cache clears; a generation check discards results from before reset.
- Failed fetch/decode attempts back off for 30 seconds per last failed source,
  across pans. Successful empty/transparent images are reusable coverage gaps.

### `decode_shade`

- **Does**: Preserves DNR grayscale relief and its alpha. USGS retains the
  existing style where lit ground is transparent and shadow is dark.
- **Rationale**: DNR's actual grayscale detail must be visible on the default
  black map canvas; painting only black shadows there would hide the product.
- Preserves input alpha so missing DNR coverage never becomes black shadow.
  PNG dimensions are checked before allocation, with a 2048-pixel edge cap.

## Contracts

| Dependent | Expects | Breaking changes |
|---|---|---|
| `local_terrain_scene` | Drawing is cheap enough for every frame and never blocks on the network | Fetching inline in `draw_hillshade` |
| `contour_asset::clear_caches` | `clear` drops the texture | Holding the texture past a cache reset |

## Notes

- Offline tests cover source changes, transparent nodata, image size rejection,
  post-clear worker completion, grayscale visibility and failure backoff in
  addition to bounds reuse.

- The texture is fetched for the view's bounds, and a pan is tolerated up to
  `REUSE_FRACTION` of the view before the stale image is dropped. That keeps a
  slow drag from streaming a new image every frame while avoiding a texture
  from a completely different area.
- Row 0 of the service's image is the northern edge, which is why the drape maps
  `v = 0` to `max_lat`.
- The local scene checks Earth mode and its in-memory streaming toggle before
  drawing; workers check the saved toggle before fetching. Geography screens
  reject remote views without network work.
