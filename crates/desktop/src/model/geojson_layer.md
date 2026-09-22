# geojson_layer.rs

## Purpose
Defines the in-memory vector overlay model used by the desktop map renderer and parses user-uploaded layer files into that model. Despite the historical name, this file now covers generic uploaded vector layers, not only raw GeoJSON.

## Components

### `GeoJsonGeometry`
- **Does**: Enumerates the geometry primitives the renderer knows how to draw for uploaded overlays
- **Interacts with**: Globe and local-terrain overlay drawing code in `panels/world_map`

### `GeoJsonFeature`
- **Does**: Stores one parsed geometry, its best-effort display label, and an
  optional per-feature colour taken from the source's own styling properties
- **Interacts with**: `GeoJsonLayer`, tooltip/detail rendering

### `GeoJsonLayer`
- **Does**: Holds one togglable overlay layer — uploaded or built-in — and exposes GeoJSON/KML/KMZ parse entrypoints that normalize all supported formats into the shared geometry model. `show_labels` controls whether its feature labels draw
- **Interacts with**: `header.rs` import flow, world-map layer toggles, `kml_layer.rs`, `submarine_cables.rs`

### `extract_color` / `parse_hex_color`
- **Does**: Read a per-feature colour from a `color`/`colour` property or the simplestyle-spec `stroke`/`marker-color`/`fill` keys, accepting `#rgb` and `#rrggbb`
- **Interacts with**: `collect_features`, both world-map overlay renderers

### `ring_centroid`
- **Does**: Computes a simple average centroid for polygon-ring label placement and hit-testing helpers
- **Interacts with**: World-map overlay rendering helpers

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `header.rs` | Uploaded layer parsing returns a ready-to-render `GeoJsonLayer` or a user-facing error string | Removing `parse_upload` or changing format support without updating the importer |
| `world_map` renderers | Geometry variants and feature labels stay stable across imported formats | Renaming geometry variants or changing coordinate semantics |
| `kml_layer.rs` | Parsed KML features map cleanly into the same `GeoJsonFeature` / `GeoJsonGeometry` types | Introducing format-specific geometry types into the shared overlay model |

## Notes
- Per-feature colour takes precedence over the layer colour; `None` (the common
  case for uploads) falls back to it. Alpha is fixed at the layer palette's 220
  so a styled feature cannot render more opaque than the rest of its layer.
- `show_labels` defaults to `true` so uploads are unchanged. Built-in layers
  that carry thousands of features start it `false`: the globe view has no
  viewport culling for labels, unlike the local view.
- The single-layer color palette is intentionally format-agnostic. KML/KMZ style/icon fidelity is not preserved yet; imported KML placemarks are normalized into the renderer’s existing per-layer color treatment.
