# core_import_tests.rs

## Purpose
Verify durable Earth imports keep core crossings, discard halo-only features,
store correct contour/coastline manifest counts and roll back corrupt input.

## Contracts
Creates isolated temporary databases; verifies existing legacy entries remain
byte-identical alongside new core entries. Empty cores commit as known empty.
