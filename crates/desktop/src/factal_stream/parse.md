# parse.rs

## Purpose
Normalize a single Factal API item into a geolocated `EventRecord`. Kept separate
from pagination and polling so live and replay loading use the same translation.

## Components
- `parse_event` preserves stable Factal IDs, severity, timestamps, topic location,
  content and raw JSON for the brief. Items without a topic location are skipped.
- Private conversion helpers handle numeric/string IDs and coordinates, severity
  categories and whitespace normalization.

## Contracts
| Dependent | Expects | Breaking changes |
|---|---|---|
| `api.rs` | `parse_event` returns only geolocated events with `factal-` IDs | ID format or dropping brief data |

## Notes
Extracted from the original stream module without changing its field mapping.
