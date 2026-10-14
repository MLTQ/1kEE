# globe_manifest.rs

## Purpose
Keep Earth/Moon/Mars globe root discovery, SQLite manifest reads and terrain build
scheduling off the drawing thread. Camera frames only consult a snapshot.

## Contracts
One worker per globe cache, retained across resets. Results require both epoch
and exact root/tier/window identity. A 350 ms refresh also discovers external
cache-builder writes; in-process manifest revisions trigger an earlier refresh.
Revision is captured before selection so concurrent completions are not lost.
Failures release the worker gate. No disk access is performed by `assets` itself.

Body-specific grid extents and zoom buckets key each independent cache.

This loader owns repaint scheduling for globe terrain; the panel does not need
a separate synchronous database status check. A regression simulates blocked
manifest I/O and a completed empty snapshot, verifying both keep scheduling
refresh frames without opening any database on the UI path. Worker completion
also requests an immediate repaint through the existing callback.
