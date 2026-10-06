# native_contours.rs

## Purpose
Identify native SRTM geometry independently of filenames and reject accidental
reuse of older coarse caches. Shared by runtime and offline builders.

## Components
- `is_native` checks a quality tag without writing to the database.
- `initialize` tags empty schemas and accepts already tagged caches. Untagged
  nonempty contour/coastline manifests return an error without replacing data.

## Contracts
Call initialization after the common contour schema exists and before scheduling
native builds. Readers only check the tag. Tests verify fresh-cache admission,
continued writes to tagged caches, and rejection of legacy manifests.
