# submarine_cables.rs

## Purpose

Provides a cache-first, public-data-only adapter for the TeleGeography
submarine cable map. It supplies cable routes and landing points as ordinary
overlay layers without owning any rendering — the shared `GeoJsonLayer`
pipeline draws them.

## Components

### `tick` / `shutdown`
- **Does**: Schedule cache reads and a bounded HTTP refresh off the UI thread,
  and retire worker results during app shutdown.
- **Interacts with**: `app.rs` and `AppModel`.

### `load` / `fetch` / `build_layer`
- **Does**: Resolve a fresh cache, then the two public endpoints, then a stale
  cache; parse each payload into a named built-in layer and reject a response
  too small to be genuine.
- **Interacts with**: `model::GeoJsonLayer::parse`, `reqwest`.

### Cache helpers (`cache_dir`, `read_cache`, `write_cache`)
- **Does**: Resolve the cache location the same way `deflock_source` does, read
  both payloads together, and persist them through temporary files.
- **Interacts with**: `settings_store`, the effective asset/data root.

### Provenance constants
- **Does**: Supply TeleGeography attribution text and link for the layer
  drawer.
- **Interacts with**: `panels/layer_drawer.rs`.

## Contracts

| Dependent | Expects | Breaking changes |
|---|---|---|
| `AppModel` | Layers arrive fully parsed and ready to render | Emitting partially-parsed layers, or one of the pair without the other |
| `app.rs` | `tick` is non-blocking and does nothing while the layer is off | Performing HTTP or disk work on the UI thread, or fetching before the operator opts in |
| World-map renderers | Input is a plain `&[GeoJsonLayer]`, same as user uploads | Introducing a cable-specific geometry or renderer path |
| `layer_drawer` | Toggling labels mutates only `show_labels` on the existing layers | Rebuilding or refetching layers to change a display flag |

## Notes

- The display toggle defaults off, and **nothing is fetched until it is turned
  on** — a session that never opens the layer does no network or disk work.
- Both payloads are cached together and only written once both parsed, so a
  half-written pair can never be read back as a complete snapshot.
- Cables carry a per-cable `color` property, which `GeoJsonFeature::color`
  now honours; the layer colour is only a fallback.
- Built-in layers start with `show_labels: false`. The globe view has no
  viewport culling for labels, and ~2,600 cable and landing names drawn at once
  is unreadable. The names stay on the features for the drawer's label toggle
  and any future hit-testing.
- A stale cache is preferred over an empty layer when the network fails, so the
  overlay keeps working offline.
