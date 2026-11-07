# recovery_tests.rs

Disposable SQLite tests for terrain recovery. Invalid blobs, missing rows, stale
reader errors and actual overwritten index-page bytes exercise deletion, rollback,
persistent isolated replacements, healthy-neighbor preservation and rebuilding.
No test touches an installed terrain cache; manual maintenance uses the explicit
`--repair-native-tile` command documented in recovery.md.
