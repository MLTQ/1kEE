# http.rs

## Purpose
Bound network transfers and catalog memory for the public elevation adapters.

## Contracts
Downloads stream through a 64 KiB buffer with a 64 MiB ceiling and report actual
bytes. Caller-owned scratch directories remove failures. JSON responses are
limited to 4 MiB and cached for ten minutes within 16 MiB. PNG reads have their
own smaller caller-supplied limit. HTTP errors are retryable except explicit 404.
Catalog-derived URLs must use HTTPS and one of the official provider hosts;
local paths, credentials and arbitrary hosts are rejected before GDAL use.

The enclosing acquisition deadline also bounds HTTP bodies and cached metadata
traversal; a missing source is never inferred from a timeout.
