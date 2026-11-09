# atlas_viewport.rs

## Purpose
Choose stable geographic detail windows from the actual Globe or Local viewport.
Arcsecond edges permit fine native sampling without a one-degree minimum crop.

## Components
- `globe` samples the canvas and perspective limb, unwraps longitude around the
  view centre and includes visible poles. Small globes retain the overview.
- `local` inverts all four real clip corners through the sea-level projector,
  including yaw/pitch and asymmetric layout, rather than guessing a view radius.
- `Region` pads/snaps bounds to a power-of-two arcsecond grid. `size` keeps all
  native samples when they fit, with an 8192-edge / 32-million-pixel ceiling.

## Contracts
Bounds are west/south/east/north pixel edges; longitudes may cross ±180 degrees.
Selection performs no I/O. Texture limits also honor the current GPU limit.
Padding includes room for terrain lighting samples and modest camera movement.
