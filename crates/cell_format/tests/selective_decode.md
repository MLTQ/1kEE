# selective_decode.rs

## Purpose
Ensures unrelated chunks are skipped without decoding, while malformed requested
data and absurd feature counts fail safely before allocation.
