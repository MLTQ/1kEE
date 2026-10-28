# boundaries.rs

Public in-process PBF boundary importer for the main desktop application. Uses
the existing relation/way scans, compact original-PBF coordinate index, polygon
assembly, and atomic level-file writer. Progress is synchronous to the caller;
the caller must run it on a background worker.

`load` holds the compact index's OS lock throughout extraction, validates source
identity, and reuses compatible scan checkpoints. It does not bind or finish
general way-build options, so a boundary-only import cannot mark unrelated roads
complete. Output is `<output>/admin_cells`, scratch is
`<output>/.planet_build/indexed-pbf-v1`.

`is_running` probes the actual lock read-only: an abandoned lock file is not an
active job. Errors remain errors. The definitive writer lock is still acquired
inside `load`, preventing races with companion builds.
