# planet_join_io.rs

16-byte record spools with SHA-256 segment verification and bounded external sort.
Append buffers are 64 KiB per touched destination; files are opened only during
flush/sync, avoiding a descriptor per planet way group. Manifest segment lengths
are authoritative on restart; truncated committed files fail and trailing
uncommitted bytes are truncated. Reads verify complete segments while streaming; callers publish results only
after consuming and verifying all input. Empty spools need no physical file.

Sorting uses 64 MiB chunks and at most 32 input streams per merge pass. All runs
are checksummed too. Reconstructible sort directories can be removed on retry;
source request spools are kept until their join partition commits. Merge output
emits activity callbacks during long passes. This layer never mutates the PBF,
existing node index, feature checkpoint or published cell files.
