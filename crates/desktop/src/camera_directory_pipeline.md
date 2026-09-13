# camera_directory_pipeline.rs

## Purpose

Runs 1kEE's explicit opt-in public camera-directory pipeline. It adapts the
useful Project Eyes On stages—directory discovery, deduplication, feed
classification, and geolocation—without importing its broad search-engine
dorking path.

## Components

### `EyesOnPipelineConfig`

- **Does**: Carries the optional ISO country scope and normalized
  requests-per-minute pace.
- **Interacts with**: `camera_registry.rs` and persisted app settings.

### `fetch`

- **Does**: Walks through Insecam's source-advertised page count, with
  empty/repeated-page fallbacks, deduplicates advertised public-IP URLs, reuses
  fresh metadata, enriches cache misses concurrently, and returns normalized
  `CameraFeed` records.
- **Interacts with**: `CameraFeed` in `model/cameras.rs` and the shared blocking
  HTTP client owned by `camera_registry.rs`.
- **Rationale**: Network work remains off the UI thread; request starts are
  globally paced even while camera enrichment is concurrent.

### `EyesOnPipelineProgress`

- **Does**: Reports completed directory pages, discovered candidates, reused
  results, completed feed checks, geolocated records, and reachable feeds.
- **Interacts with**: the registry progress channel in `camera_registry.rs`.
- **Rationale**: Operators can distinguish a narrow/low-yield scan from a scan
  that is still working.

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
| `camera_registry.rs` | `fetch` is blocking but runs only inside the registry worker and reports progress through a thread-safe callback | Calling it on the UI thread or blocking the progress callback |
| Map/model consumers | Every returned camera has valid geographic coordinates and a stable `eyes-on-*` id | Returning ungeolocated records or unstable ids |
| Settings UI | The pipeline is off by default and its 10–3,000 requests/min input is normalized before use | Implicit enablement or bypassing paced request starts |
| `runtime.rs` | All directory, detail, and feed requests acquire one shared paced slot and long scans honor cancellation | Starting HTTP requests outside the pacer or ignoring cancellation |

## Notes

- The direct feed probe reads response headers only; it does not retain image or
  video payloads.
- Pagination has no fixed 1kEE page/count ceiling. It follows the page total in
  Insecam's `pagenavigator` markup; the first empty or fully repeated page also
  ends the crawl, and HTTP 404/410 responses are treated as the end.
- Completed directory crawls are reused for 30 minutes; successful camera
  enrichment is reused for six hours and negative results for 30 minutes.
- Insecam documents its coordinates as approximate. 1kEE preserves them as
  source metadata rather than presenting them as surveyed positions.
- Project Eyes On is MIT-licensed by Y0oshi; this implementation is an original
  Rust adaptation of its pipeline design.
