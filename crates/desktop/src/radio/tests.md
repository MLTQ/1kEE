# tests.rs

## Purpose
Checks realistic directory parsing, exclusion of misleading music/provider
matches, geographic precision, public URL validation and saved-stream persistence.

## Contracts
- Ordinary tests never contact a provider or emit audio.
- `radio_live_catalog_smoke` is ignored by default and explicitly exercises the
  actual network adapter when requested; it reads metadata only.
  Its output includes provider totals and weather-only versus other category
  totals, keeping weather broadcasts distinct from scanner/dispatch growth.
