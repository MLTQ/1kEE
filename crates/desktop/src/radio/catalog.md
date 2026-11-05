# catalog.rs

## Purpose
Queries Radio Browser's public, app-compatible directory only when requested.
It retrieves a bounded safety/dispatch subset and preserves unknown locations.

## Components
- `fetch` discovers and rotates mirrors, searches explicit categories/names,
  validates metadata and deduplicates by UUID and resolved stream URL.
- `classify` excludes ordinary music matches such as the band The Police.
- `record_play` sends the directory's required click notification after Play.

## Contracts
| Dependent | Expects | Breaking changes |
|---|---|---|
| Radio panel worker | Bounded responses/timeouts and cancellation between requests | Doing this work on the UI thread |
| Geographic filter | Nullable coordinates remain nullable | Guessing coordinates from names |
| Provider | Descriptive User-Agent, mirror discovery, click accounting | Polling while idle or scraping restricted catalogs |

## Notes
Searches cap at 500 results each; a full page or failed subquery is reported as
incomplete. Catalog coverage is sparse and community-maintained. Metadata does not
establish coverage, an official dispatch affiliation or connection to an incident.
References: https://api.radio-browser.info/ and https://docs.radio-browser.info/.
