# camera_scrape_catalog.rs

## Purpose
Loads a curated scraped-camera seed catalog for public webcam directory pages that do not expose a stable API. This keeps brittle HTML source definitions declarative and operator-controlled.

## Components

### `ScrapedCameraSourceKind`
- **Does**: Enumerates the lightweight page-parser variants supported by the scrape adapter path
- **Interacts with**: `camera_registry.rs`

### `ScrapedCameraSource`
- **Does**: Stores one curated scrape seed, including the source page URL and optional operator-supplied coordinates when the page markup is weak or untrustworthy
- **Interacts with**: `camera_registry.rs`

### `load_scrape_sources`
- **Does**: Reads `Data/camera_sources/scrape_sources.json` under the selected asset root and returns only enabled source definitions
- **Interacts with**: `camera_registry.rs`
- **Rationale**: Keeps the first scrape path curated and predictable instead of pretending we have a reliable crawler

## Notes
- Coordinates are still preferred in the catalog because public webcam directories often have poor or inconsistent geolocation metadata, but they are now optional when the page exposes a strong embedded-map signal.
- This is a seed list, not a crawler. Operators choose which public pages to trust and enable.
- Provider-specific parser kinds exist so the registry can grow from generic HTML extraction into stronger site-specific handling later, including map-based coordinate extraction for pages like `worldcams.tv`.
