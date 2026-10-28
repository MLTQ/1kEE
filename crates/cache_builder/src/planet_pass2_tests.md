# planet_pass2_tests.rs

Real ordinary/dense PBF regressions assert zero node-block decompressions when
no selected way matches, correct scanned-way counts and progress without output
flushes. Selecting roads still resolves coordinates and emits the fixture road.
All indexes, checkpoints and source data live in owned temporary directories.
