# fire_layer.rs

## Purpose

Batched visual treatment for NASA FIRMS active-fire detections. Geographic
projection stays in the globe/local scene that knows the active camera; this
module receives screen-space positions plus intensity and emits one mesh.

## Components

### `ProjectedFire`
- **Does**: Hold a detection that has already been projected into the current
  scene, plus the intensity and confidence the mark encodes.
- **Interacts with**: `globe_scene::draw_active_fires`,
  `local_terrain_scene::draw_active_fires`.

### `sanitize_frp`
- **Does**: Clamp fire radiative power into the domain the size/colour curves
  expect, so a negative or NaN value can never reach `ln`.
- **Interacts with**: `radius_for`, `color_for`.

### `radius_for` / `color_for`
- **Does**: Map FRP onto a marker radius and a heat colour along the same
  logarithmic curve, so size and colour always agree.
- **Interacts with**: `build_mesh`.

### `build_mesh` / `draw_mesh`
- **Does**: Build every supplied detection into a single mesh, and submit a
  previously-built one.
- **Interacts with**: the scene mesh caches keyed on snapshot revision and
  camera transform.

## Contracts

| Dependent | Expects | Breaking changes |
|---|---|---|
| Globe/local scenes | `build_mesh` output is cacheable and depends only on its inputs | Reading global or time-varying state inside the builder |
| Scene mesh caches | Identical inputs produce identical pixels | Introducing randomness or frame-dependent styling |
| `fire_source` | Rendering never mutates or refetches detection data | Moving fetch/cache ownership into the layer |

## Notes

- Marks are axis-aligned quads, not discs: four vertices per mark matters at
  tens of thousands of detections. `build_mesh` emits 2 quads for a nominal
  detection and 3 for a high-confidence one.
- FRP is extremely skewed (median ~5 MW, daily max several hundred), so the
  curve is logarithmic — ordinary agricultural burning stays legible without a
  single megafire dominating the view.
- `f32::clamp` **propagates** NaN rather than rejecting it, so clamping alone
  would not protect the mesh from a NaN radius. `sanitize_frp` guards the input
  instead. Callers also clamp, but the helpers are total on their own.
- The orange/white heat palette is deliberately distinct from the violet ALPR
  markers and the event severity colours.
- High-confidence detections get a hot core so they read differently from the
  nominal-confidence field without needing a separate legend.
