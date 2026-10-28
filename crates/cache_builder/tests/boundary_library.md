# boundary_library.rs

Exercises the exact shared API used by the desktop with the synthetic admin PBF.
Checks all level caches, progress, exact resume output, unchanged way-build state,
mutual exclusion with the companion, abandoned lock files, and source mismatch
preserving the existing output. All writes stay in an owned temporary directory.
