# Build monitoring

Planet builds emit structured source-scan counters with their existing progress
messages. `build-status.json` in the selected working directory is atomically
replaced at most once per second, plus a final completed/failed record. It stores
source/output paths, elapsed time, stage, message and the last measured counters.
Both CLI and GUI builds use this reporter. It is observational; resume offsets
and output formats are unchanged. Report write failures warn without aborting a
cache build. A stale `running` report does not establish process liveness.

Rates use a rolling window of newly processed bytes. Resumed bytes are not
credited to this session's throughput. Phase changes/backward offsets reset the
window. A stale or finished worker suppresses rate/ETA. ETA estimates traversal
to source EOF for this scan, not the entire build or geographic coverage.
Checkpoint bytes advance only after the corresponding existing checkpoint has
been saved. The compact-index progress callback follows its SQLite commit.
Way counters, selected-feature counts and file-write counts are session totals;
file writes can revisit the same geographic cell. Completed terrain tile events
also feed a separate tiles/minute counter in the GUI.

Tests cover phase/resume rate math, generated cache parity, telemetry on success
and failure, and preservation of durable state during rejected/no-op compact
builds. `build-status.json` is intentionally excluded from the latter bytewise
comparison; cache and checkpoint files remain covered.

When a compact build rejects source identity or settings, the GUI offers an
explicit fresh-working-folder action. It creates a unique empty child directory
and selects it for the next build; it does not start scanning, overwrite old
checkpoints or relax source identity checks. Existing feature output remains at
the selected cache destination. Copying a PBF between filesystems can change
inode/timestamp precision even when the sampled fingerprint matches; that sample
is not a full-file content proof.

Way batches run under a scoped worker while the coordinating thread emits a
one-second heartbeat. Per-blob completion counts and successfully decoded
coordinate block/byte counters update before the 64-blob barrier finishes.
These bytes include repeated reads and OS cache hits, not physical disk traffic.
Source/checkpoint byte positions never advance on a heartbeat. Partial batch
feature counters are explicitly not durable. A separate last-measured-progress
age changes only when counters change; a responsive heartbeat cannot hide an
I/O stall. Worker disconnection/panic terminates the wait rather than leaving a
spinner alive indefinitely. The existing single batch and Rayon limits remain.
