# camera_scrape_catalog.rs

## Purpose
Loads a curated scraped-camera seed catalog for public webcam directory pages that do not expose a stable API. This keeps brittle HTML source definitions declarative and operator-controlled.

## Components

### `ScrapedCameraSourceKind`
- **Does**: Enumerates the lightweight page-parser variants supported by the scrape adapter path
- **Interacts with**: `camera_registry.rs`

### `ScrapedCameraSource`
- **Does**: Stores one curated scrape seed, including the source page URL and the operator-supplied coordinates that make it usable even when the page markup is weak
- **Interacts with**: `camera_registry.rs`

### `load_scrape_sources`
- **Does**: Reads `Data/camera_sources/scrape_sources.json` under the selected asset root and returns only enabled source definitions
- **Interacts with**: `camera_registry.rs`
- **Rationale**: Keeps the first scrape path curated and predictable instead of pretending we have a reliable crawler

## Notes
- Coordinates are intentionally explicit in the catalog because public webcam directories often have poor or inconsistent geolocation metadata.
- This is a seed list, not a crawler. Operators choose which public pages to trust and enable.
- Provider-specific parser kinds exist so the registry can grow from generic HTML extraction into stronger site-specific handling later.
