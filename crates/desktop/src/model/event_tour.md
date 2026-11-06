# event_tour.rs

## Purpose
Choose varied stops from every eligible event in the live Factal window, with
more frequent visits to higher-severity stories and a cooldown on repeats.
No camera, rendering, disk or network work lives here.

## Components
- `refresh` ranks all unique geolocated `factal-` records by numeric severity,
  newest timestamp and stable ID. Category severity is the fallback. It retains
  waiting credit and recent visits by ID while refreshing metadata/removing old
  records. New or revised headlines do not interrupt the current stop.
- `take_pending` starts the first tour stop immediately, or waits for real data.
- `next` uses smooth weighted round robin over eligible records: severity ≤2 has
  weight 1, severity 3 weight 3, severity 4 weight 6, severity ≥5 weight 10.
  Waiting records accumulate credit; the selected record pays the sum of eligible
  weights. Cooldown entries freeze their credit. This gives low-priority records
  eventual turns instead of repeatedly choosing just the highest-ranked IDs.
- The current target is never selected again immediately. A large feed requires
  six intervening stops before a repeat. Small pools use half their alternatives
  as the cooldown so they can continue cycling; one record stays in orbit.
- `position` exposes severity rank within the entire eligible feed, independently
  of visit order.

## Contracts
| Dependent | Expects | Breaking changes |
|---|---|---|
| `event_follow.rs` | Only initial startup is pending immediately; other stops wait for normal flight/dwell completion | Interrupting a stop on every new top headline |
| Brief | `position` is priority rank/count, not sequential tour progress | Truncating the pool or implying sequential traversal |
| `event_tour_tests.rs` | Deterministic choices and persistent state across refreshes | Random resets or unstable tie breaking |

## Notes
Weights express relative preference, not exact frequency promises: repeat spacing
and pool size also affect visit frequency. Stopping follow discards visit history.
