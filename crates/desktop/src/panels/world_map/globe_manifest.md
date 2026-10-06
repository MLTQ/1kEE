# globe_manifest.rs

## Purpose
Keep Earth globe root discovery, SQLite manifest reads and terrain build
scheduling off the drawing thread. Camera frames only consult a snapshot.

## Contracts
One worker per globe cache, retained across resets. Results require both epoch
and exact root/tier/window identity. A 350 ms refresh also discovers external
cache-builder writes; in-process manifest revisions trigger an earlier refresh.
Revision is captured before selection so concurrent completions are not lost.
Failures release the worker gate. No disk access is performed by `assets` itself.
