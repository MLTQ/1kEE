# main.rs

Thin executable entry point. Delegates to the shared library's `run`, printing
errors to stderr and exiting nonzero on failure. CLI and GUI dispatch live in
`lib.rs`; library consumers do not launch a companion process.
