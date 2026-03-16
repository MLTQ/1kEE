# terrain_field.rs

## Purpose
Provides the procedural fallback terrain signal used by the globe renderer. Real streamed or derived terrain now takes precedence, but this file remains the safety net if runtime assets are missing.

## Components

### `elevation`
- **Does**: Returns a synthetic elevation-like scalar field over the globe
- **Interacts with**: `globe_scene.rs`
- **Rationale**: Keeps the renderer resilient when the derived runtime raster is unavailable

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `globe_scene.rs` | `elevation` is deterministic and cheap enough for per-frame sampling | Making the field expensive or non-deterministic |

## Notes
- This file now acts as the fallback beneath `terrain_raster.rs`.
