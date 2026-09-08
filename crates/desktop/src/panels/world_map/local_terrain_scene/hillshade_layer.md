# hillshade_layer.rs

## Purpose

Drapes a server-rendered 3DEP multidirectional hillshade over the local terrain
surface. This is distinct from the elevation fill, which shades a 60×60
interpolated mesh: the image service renders from its own 1 m source, so the
relief detail is finer than the fill mesh can express.

## Components

### `draw_hillshade`

- **Does**: Builds a 48×48 drape mesh over the cached texture's bounds,
  projecting each vertex through `project_local` at its surface elevation, and
  paints it as a single textured mesh.
- **Interacts with**: `projection::project_local`, `ElevationSurface`,
  `local_terrain_scene::render_local_terrain_scene`.
- **Rationale**: Draping on the projected surface keeps the shade registered
  with the contours and markers instead of floating as a flat overlay.
  Unprojectable vertices drop only the cells that touch them.

### `ensure_requested` / `upload_pending`

- **Does**: Schedules one background fetch when the view leaves the cached
  bounds, then uploads the decoded image to a texture on the UI thread.
- **Interacts with**: `threedep::fetch_hillshade_png`.
- **Rationale**: Texture upload needs the egui context, so the fetch thread can
  only hand back a decoded `ColorImage`.

### `decode_shade`

- **Does**: Converts the greyscale PNG into a premultiplied image whose alpha
  carries the shading — lit ground is transparent, shadow is dark.
- **Rationale**: Painting darkness as alpha rather than opaque grey keeps the
  contour and fill colours below visible through lit slopes.

## Contracts

| Dependent | Expects | Breaking changes |
|---|---|---|
| `local_terrain_scene` | Drawing is cheap enough for every frame and never blocks on the network | Fetching inline in `draw_hillshade` |
| `contour_asset::clear_caches` | `clear` drops the texture | Holding the texture past a cache reset |

## Notes

- The texture is fetched for the view's bounds, and a pan is tolerated up to
  `REUSE_FRACTION` of the view before the stale image is dropped. That keeps a
  slow drag from streaming a new image every frame while avoiding a texture
  from a completely different area.
- Row 0 of the service's image is the northern edge, which is why the drape maps
  `v = 0` to `max_lat`.
- The layer is Earth- and local-mode only, and does nothing when 3DEP is
  disabled in settings.
