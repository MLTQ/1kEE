# roads_global.rs

## Purpose
Imports and reads legacy global road tiles in SQLite. Loaded legacy rows carry no baked elevations; the renderer samples terrain only when needed.

## Contracts
Existing geometry encoding, source classification and tile conventions remain unchanged.

- Missing-cell fallback reuses one connection and prepared statement across the cell list.
