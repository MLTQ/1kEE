# camera_source_catalog.rs

## Purpose
Loads the optional public/no-key camera source catalog for the desktop app. This module keeps source definitions declarative so new public adapters can be added without touching the core registry loop every time.

## Components

### `PublicCameraSourceKind`
- **Does**: Enumerates the supported generic no-key source shapes: plain JSON arrays, GeoJSON feature collections, and ArcGIS feature services
- **Interacts with**: `camera_registry.rs`

### `PublicCameraSource`
- **Does**: Stores one declarative public-source definition, including endpoint, parser kind, and field mappings
- **Interacts with**: `camera_registry.rs`

### `load_public_sources`
- **Does**: Reads `Data/camera_sources/public_sources.json` under the selected asset root when present and returns only enabled source definitions
- **Interacts with**: `camera_registry.rs`
- **Rationale**: Lets operators or future build steps add no-key public sources without recompiling the app

## Notes
- The catalog is optional; if no file is present the app simply runs without public no-key sources.
- The initial goal is flexibility, not a universal schema. Field mappings stay explicit per source so brittle public endpoints can be adapted incrementally.
