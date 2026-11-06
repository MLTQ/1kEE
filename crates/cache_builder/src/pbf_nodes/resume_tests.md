# resume_tests.rs

Regression coverage for external-volume remounts. Simulates changed mount device
numbers in both legacy and fingerprinted indexes, checks exact lookup parity,
preserved build options/offsets and unrelated way checkpoints. Tests actual source
bytes changed with restored metadata, ordinary inode/size/mtime rejection, and
legacy descriptor/source mismatches. Rejections must preserve the database bytes.

An ignored real-data check accepts explicit PBF/index environment paths, takes
the existing build lock read-only, copies only the small SQLite index to a unique
temporary directory and verifies recovery against the source. It never rewrites
the production index or restarts extraction.
