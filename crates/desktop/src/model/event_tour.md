# event_tour.rs

Owns the six-stop snapshot and next-stop cursor for `event_follow`. It does no
camera, disk or network work and only accepts actual geolocated Factal records.

- Sort by descending numeric Factal severity (falling back to the normalized
  severity when missing), then newest timestamp, then stable ID. Deduplicate IDs
  before taking six. Missing/invalid coordinates and non-Factal records are excluded.
- Every successful payload refreshes the ranking and metadata. A changed highest
  priority ID is pending immediately; an unchanged head preserves the current
  stop and advances from its position in the refreshed order. This prevents
  routine polls from repeatedly restarting the tour at stop one.
- `next` cycles through the latest snapshot and skips the currently followed ID.
  A single stop stays in orbit; an empty snapshot schedules no further stops.
- `position` exposes the current severity rank and number of stops for the brief.
  Stopping event follow discards the tour; enabling builds from the current feed.
