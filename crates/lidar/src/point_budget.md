# Shared Target loading limits

Carries one deadline and counters through all source readers and refinement
passes: 96 million decoded record visits, 4096 node/file reads and 256 MiB of
network transfers within a 180-second loading window. Cached source bytes do
not count as network transfers. Decode read-ahead is counted, including a tail
read again after resuming a compressed record stream.

Fetch reads respect remaining transfer allowance (with one sentinel byte for
oversize detection), and HTTP timeouts use the remaining loading time. EPT and
bulk readers use the remaining decoded-record allowance for each batch. Limits
are conservative resource guards and never represent measured area coverage.
