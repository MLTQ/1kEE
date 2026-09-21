# storage.rs

## Purpose
Pauses background terrain generation before a nearly-full cache volume causes
GDAL schema creation failures and cascades of `cannot write linestring` errors.

## Components
- `require_room`: checks the nearest existing parent with POSIX `df -Pk` and
  requires 1 GiB free before source work and before contour generation. Emits
  a clear low-space diagnostic at most once per 30 seconds.
- `available_bytes`: parses available blocks independently of capacity and
  mount paths containing spaces; invalid/unavailable output stays unknown.

## Contracts
| Dependent | Expects | Breaking changes |
|---|---|---|
| GDAL tile workers | Failure returns through existing retry backoff and RAII cleanup | Performing the check on the UI thread |
| Cached geometry readers | Continue reading existing tiles on full volumes | Applying admission to read-only paths |

## Notes
This is a best-effort preflight, not an atomic reservation: other processes or
exceptionally large tiles can consume space afterward. Actual I/O errors remain
authoritative. On platforms without `df`, unknown space preserves prior behavior.
No user files or persistent cache entries are deleted by this module.
