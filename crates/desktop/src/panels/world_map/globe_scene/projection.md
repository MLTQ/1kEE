# projection.rs

## Purpose

Projects geographic points and linework through the globe camera transform.
It centralizes the unit-sphere contract shared by the GPU globe backdrop and
interactive globe overlays.

## Components

### `project_geo`

- **Does**: Projects generic terrain-aware overlays using the existing
  synthetic terrain-radius behavior.
- **Interacts with**: geography, labels, and generic marker helpers.

### `project_geo_unit_surface` / `project_geo_unit_surface_elevated`

- **Does**: Projects event, camera, and replay bases on the visible unit
  sphere, with the elevated form extending strictly outward for beam tips.
- **Interacts with**: `globe_scene/mod.rs` event and camera rendering.
- **Rationale**: The GPU globe shades terrain on a unit sphere rather than
  geometrically displacing it, so interactive bases must not use a negative
  terrain radius that would place their screen indicator inside the sphere.

### `GlobeXform`

- **Does**: Caches a frame's yaw/pitch/perspective transform per thread.
- **Interacts with**: all projection helpers and Rayon-capable path rendering.

## Contracts

| Dependent | Expects | Breaking changes |
|---|---|---|
| `globe_scene/mod.rs` | Event/camera hit-test positions match their rendered surface bases | Changing unit-surface or outward-tip semantics |
| `globe.wgsl` | CPU surface projection uses the same unit-sphere camera geometry | Changing rotation or perspective math |
| Geography helpers | Generic terrain-aware and flat projections retain their existing behavior | Altering `project_geo` or `project_geo_flat` radius semantics |

## Notes

- Vector coastlines and contours are rendered in small positive shells, but
  paint order keeps interactive surface indicators above them without requiring
  a larger marker radius.
