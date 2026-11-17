# globe_loading_metadata.rs

Builds Earth Globe loading snapshots from existing contour-cache metadata.
Manifest workers capture exact root/tier/window identity, source availability,
ready/source-free cells, selected assets and atomic build-progress handles.
Drawing reads these snapshots with a nonblocking cache lock.

Only currently wanted cells contribute. Builds use measured source/generation/
commit fractions; cached assets start at 75%, decoding advances toward 99%, and
decoded geometry waits at 99% until composition publication. Known empty cells
finish without inventing terrain. Downstream instance/GPU stages report
separately. Read failures in backoff count as unavailable. Overview snapshots
report readiness for the exact requested product key while retaining drawable
older data.

An old manifest cannot describe a new viewport or reset cache. Periodic refresh
of a completed current manifest does not reactivate loading. Regression coverage
in `globe_manifest` exercises decoding, publication, refresh, pan and reset.
