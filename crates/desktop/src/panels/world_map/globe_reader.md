# globe_reader.rs

## Purpose
Stream Earth globe tiles into the composition pipeline one at a time, allowing
the center of a large cached region to appear before all neighbors finish.

## Contracts
One reader and connection per globe batch. Bounds/cost scans run on the reader.
Per-tile publication keeps the batch gate occupied, checks epoch/current view,
and wakes composition. A reset rejects publication and cancels remaining reads.
Final bookkeeping releases in-flight claims and retries failures with backoff.
Moon/Mars keep the existing batch reader. No source geometry is simplified.
