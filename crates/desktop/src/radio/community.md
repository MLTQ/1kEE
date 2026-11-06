# community.rs

## Purpose
Loads an attributed snapshot of direct streams published by NCISS and ScanBC.
Snapshot membership does not assert that a stream is currently online; playback
reports actual connection state.

## Contracts
- NCISS explicitly documents direct URLs for custom media/scanner players.
- County-specific NCISS feeds use Census representative county points labelled
  as coverage centers. Broad/regional channels remain unmapped.
- ScanBC uses the owner's published names and mounts. Bundled OpenStreetMap
  settlement points supply explicitly approximate coverage centers, not receiver
  positions. The UI credits OpenStreetMap contributors and the Census Bureau.
- No runtime geocoding or separate city database is required. The small one-time
  Nominatim extraction was cached, identified and performed sequentially below
  one request/second per https://operations.osmfoundation.org/policies/nominatim/.
- No event coordinate is assigned to a publisher feed; audio starts only on Play.
- `community_feeds.json` records sources, retrieval date and coordinate provenance.
