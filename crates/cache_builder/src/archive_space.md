# archive_space.rs

## Purpose
Reports the archive destination, temporary output size and space available on
that output volume. Captures failures before staging cleanup releases capacity.

## Contracts
- Worker-only POSIX `df -Pk` probe, at most once per second; log snapshots every
  five seconds. Reports decimal GB to match macOS volume information.
- Best-effort 1 GiB reserve prevents running blindly into a full volume. This is
  not a capacity reservation or a guarantee against large tiles/concurrent jobs.
- Unknown space does not become zero or block a pack; actual I/O errors remain
  authoritative and gain destination/temporary-size context.
- No source/cache deletion or publication occurs here. `archive_pack` owns
  staging cleanup and final publication. Error snapshots precede that cleanup.
- Tests cover volume paths with spaces, unknown results, reserve boundaries and
  capturing temporary size/destination without deleting or publishing files.
