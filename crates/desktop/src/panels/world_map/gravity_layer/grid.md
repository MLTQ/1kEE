# grid.rs

## Purpose
Read a selected regional Float32 gravity-anomaly grid and color physical mGal
values independently of the SWOT Eötvös grid contract.

## Components
- `read` checks schema, product identity, units, bounds, dimensions, missing
  value and exact file length before streaming rows into a bounded image.
- `color` uses the same diverging hues as VGG but distinct mGal ranges: ±150 free-air, ±350 Bouguer, ±100 isostatic.
  Nonfinite values and the prepared missing sentinel remain transparent.

## Contracts
`Derived/gravity/ncei/<id>` contains manifest.json and north-up little-endian
Float32 grid.bil. Bounds describe pixel edges. Zero is a valid anomaly. Images
retain all prepared samples up to the GPU limit (at most 4096 per edge); raw
input is bounded to 8192 per edge and never held as a second whole image.
All discovery, reads and color conversion run on the snapshot worker.
