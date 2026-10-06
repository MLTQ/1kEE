# gebco_depth_fill.rs

## Purpose
Build the ocean depth-fill image without disk access or per-pixel conversion on
the drawing thread.

## Contracts
A root-keyed `layer_snapshot` worker resolves/generates the BIL, reads and
validates its 1440×720 little-endian grid, builds premultiplied colors and queues
the egui texture. The UI polls a completed handle. Missing sources retry with
backoff; reset/root changes reject late results. The ocean palette and linear
sampling are unchanged, and land/nodata pixels remain transparent.
