# api_tests.rs

## Purpose
Exercise complete Factal pagination without API credentials or a running GUI.

## Coverage
- A 50-item first page plus an overlapping second page produces 76 unique events.
- Ungeolocated pages do not terminate traversal; both relative and absolute next
  links work.
- Repeated URLs and links outside the authenticated item endpoint fail before
  another request is made.
- A later-page failure fails the sync; missing/malformed results cannot clear it.
- Queries encode severity ≥2 and an exact UTC lower bound.
- An explicitly ignored live test loads the complete 24-hour range with the local
  ignored token file, prints counts only, checks unique IDs and the time window.

## Contracts
Tests inject `walk_pages` transport and never contact a service unless the live
test is explicitly invoked with `--ignored`.
