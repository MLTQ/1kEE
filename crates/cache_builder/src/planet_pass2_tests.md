# planet_pass2_tests.rs

Real ordinary/dense PBF regressions assert zero node-block decompressions when
no selected way matches, correct scanned-way counts and progress without output
flushes. Selecting roads still resolves coordinates and emits the fixture road.
All indexes, checkpoints and source data live in owned temporary directories.

Shared preparation regression compares geometry to the legacy path while forcing
multiple bounded groups. Ignored operator tests audit real dependency bytes and
benchmark old/new coordinate parity against a read-only original source using a
disposable index copy. See planet_all.md for invocation environment and limits.
