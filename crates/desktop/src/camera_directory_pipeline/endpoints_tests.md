# endpoints_tests.rs

## Purpose
Prove persistent camera reuse and scoped cache-management behavior with isolated
temporary SQLite files, never the operator's cache or camera services.

## Coverage
- Successful partial scans survive reopen; failed discoveries are not saved.
- Historical success is labeled cached rather than freshly reachable.
- Empty and arbitrarily old caches return without HTTP or directory discovery.
- Explicit refresh persists across restart and preserves last-known endpoints.
- Independent connections test removal versus late worker publication.
- Country/global scopes remain isolated; only failed endpoints are bulk-removed.
- Last-entry removal does not reactivate startup scanning.
- Public-target/coordinate validation applies on write and read.
- Corrupt caches surface an error and remain untouched.

A loopback HTTP proxy fixture drives actual discovery, cached reuse, endpoint-only
rechecking, failed removal and explicit rediscovery. Exact request counts prove
that ordinary polls and empty caches make no requests. No external host is contacted.
