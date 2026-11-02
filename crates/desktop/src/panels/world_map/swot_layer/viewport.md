# viewport.rs

## Purpose
Select an automatically refined native-grid window when the globe overview
would magnify its texels beyond a physical screen pixel.

## Components
- `detail_region` uses the actual globe camera, canvas, window size and pixel
  density. Ray/sphere samples on a 17×17 canvas grid plus 128 limb positions
  bound the visible geography, including partially visible globes.
- Longitude is unwrapped around the camera focus, preserving narrow dateline
  requests. A visible pole requests the full longitude range and polar edge.
- Eight-percent padding (at least one degree) and whole-degree snapping make
  nearby views reuse the same request and protect the sampled boundary.

## Contracts
Returns no request while the overview suffices. The request never exceeds the
native global extent and is passed to the bounded background reader. Uses the
same `screen_to_latlon`/unit-sphere projection as the real map; no disk work.
