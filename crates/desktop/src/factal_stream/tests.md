# tests.rs

## Purpose
Regress the misleading repeated Factal counts and scheduling behavior with a
headless model and explicit monotonic times.

## Coverage
- Unchanged Factal records mixed with USGS earthquakes do not generate logs or
  remove earthquakes. Same-size ID replacements and payload revisions do log.
- Initial empty results and recovery log once; subsequent empty results stay quiet.
- Failed and obsolete polls retain the previous complete feed.
- A 90-second sync gets a full minute of cooldown after completion.
- Active workers and shutdown prevent new polls; stale completion does not delay
  an immediate manually requested refresh.

## Contracts
Tests never launch source workers or modify the global scheduler; they use local
`PollManager` instances and unconfigured event persistence.
