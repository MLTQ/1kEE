# gdal_process.rs

## Purpose
Stops failed GDAL writers promptly instead of letting a missing contour table
produce thousands of errors. Preserves the first diagnostics and reaps children.

## Components
- `run`: watches process exit, shutdown, timeout, and fatal storage/contour-write
  diagnostics; kills/reaps failures and always removes their active PID.
- Diagnostic reader: drains stderr on a joined thread, reports its first eight
  lines, the first fatal write error even after that limit, and a suppressed-line
  count using the enumerated line positions. Flags full-disk or contour-write errors.
- `fatal_write_error`: recognizes specific write failures; ordinary warnings
  do not turn otherwise successful operations into failures.

## Contracts
| Dependent | Expects | Breaking changes |
|---|---|---|
| `gdal.rs` | Failed output is never imported, even if GDAL exits zero after write errors | Ignoring stderr write failures |
| Shutdown | Child is killed/reaped and diagnostic reader finishes | Detached child or unjoined pipe reader |

## Validation
Shell fixtures verify prompt cancellation/reaping, a zero exit with a missing
table diagnostic after nine warnings, and an ordinary warning. Polling is 50 ms; this is not a
promise that storage exhaustion can always be predicted before it occurs.
