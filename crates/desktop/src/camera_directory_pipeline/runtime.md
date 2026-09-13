# runtime.rs

## Purpose

Provides request pacing and short-lived reuse caches for the Project Eyes On
directory pipeline. It keeps crawl policy separate from HTML parsing and camera
normalization.

## Components

### `RequestPacer`

- **Does**: Spaces all directory, detail, and feed-probe request starts at the
  operator-selected requests-per-minute rate.
- **Interacts with**: request helpers in `../camera_directory_pipeline.rs`.
- **Rationale**: A shared pacer prevents parallel enrichment from turning a
  nominal average rate into a request burst.

### Directory cache

- **Does**: Reuses a completed country/global directory crawl for 30 minutes.
- **Interacts with**: end-of-directory discovery in
  `../camera_directory_pipeline.rs`.
- **Rationale**: The registry ticks every five minutes; repeating every listing
  page on each tick adds load without useful freshness.

### Enrichment cache

- **Does**: Reuses camera geolocation/feed results for six hours and failed
  geolocation results for 30 minutes, keyed by all candidate metadata.
- **Interacts with**: candidate enrichment in
  `../camera_directory_pipeline.rs`.
- **Rationale**: Stable detail pages and feed URLs should not be probed on every
  registry tick, while changed URLs automatically miss the cache.

## Contracts

| Dependent | Expects | Breaking changes |
|---|---|---|
| `camera_directory_pipeline.rs` | `RequestPacer::wait` is cancellation-aware and cache operations hold locks only for in-memory copies | Sleeping while holding the cache lock or ignoring cancellation |
| Registry polling | Cached directory snapshots outlive one poll but expire during a running app session | Making cache entries permanent or persisting sensitive feed data |

## Notes

- Caches are memory-only and are discarded when 1kEE exits.
- A changed camera id, URL, detail URL, brand, or location hint forces fresh
  enrichment.
