# camera_directory_pipeline.rs

## Purpose

Runs 1kEE's explicit opt-in public camera-directory pipeline. It adapts the
useful Project Eyes On stages—directory discovery, deduplication, feed
classification, and geolocation—without importing its broad search-engine
dorking path.

## Components

### `EyesOnPipelineConfig`

- **Does**: Carries the optional ISO country scope and bounded page count.
- **Interacts with**: `camera_registry.rs` and persisted app settings.

### `fetch`

- **Does**: Fetches a small set of Insecam directory pages in parallel,
  deduplicates advertised public-IP feeds, enriches them concurrently, and
  returns normalized `CameraFeed` records.
- **Interacts with**: `CameraFeed` in `model/cameras.rs` and the shared blocking
  HTTP client owned by `camera_registry.rs`.
- **Rationale**: Network work remains off the UI thread and bounded to one
  explicitly allowlisted directory.

### Directory parsing and enrichment

- **Does**: Extracts camera id, feed URL, brand, location hint, and detail page;
  reads precise coordinates from the detail page; performs a header-only feed
  reachability/type probe.
- **Interacts with**: Insecam's public listing/detail HTML and `reqwest`.

### Public-target validation

- **Does**: Accepts only credential-free HTTP(S) URLs whose host is a literal,
  publicly routable IP address.
- **Rationale**: Directory content must never turn the poller into an SSRF path
  toward loopback, LAN, link-local, documentation, or reserved networks.

## Contracts

| Dependent | Expects | Breaking changes |
|---|---|---|
| `camera_registry.rs` | `fetch` is blocking but runs only inside the registry worker | Calling it on the UI thread |
| Map/model consumers | Every returned camera has valid geographic coordinates and a stable `eyes-on-*` id | Returning ungeolocated records or unstable ids |
| Settings UI | Page scope remains within `1..=5` and the pipeline is off by default | Unbounded crawling or implicit enablement |

## Notes

- The direct feed probe reads response headers only; it does not retain image or
  video payloads.
- Insecam documents its coordinates as approximate. 1kEE preserves them as
  source metadata rather than presenting them as surveyed positions.
- Project Eyes On is MIT-licensed by Y0oshi; this implementation is an original
  Rust adaptation of its pipeline design.
