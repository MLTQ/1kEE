# geojson_layer.rs

## Purpose
Defines the in-memory vector overlay model used by the desktop map renderer and parses user-uploaded layer files into that model. Despite the historical name, this file now covers generic uploaded vector layers, not only raw GeoJSON.

## Components

### `GeoJsonGeometry`
- **Does**: Enumerates the geometry primitives the renderer knows how to draw for uploaded overlays
- **Interacts with**: Globe and local-terrain overlay drawing code in `panels/world_map`

### `GeoJsonFeature`
- **Does**: Stores one parsed geometry plus its best-effort display label
- **Interacts with**: `GeoJsonLayer`, tooltip/detail rendering

### `GeoJsonLayer`
- **Does**: Holds one togglable imported overlay layer and exposes GeoJSON/KML/KMZ parse entrypoints that normalize all supported formats into the shared geometry model
- **Interacts with**: `header.rs` import flow, world-map layer toggles, `kml_layer.rs`

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
- The single-layer color palette is intentionally format-agnostic. KML/KMZ style/icon fidelity is not preserved yet; imported KML placemarks are normalized into the renderer’s existing per-layer color treatment.
