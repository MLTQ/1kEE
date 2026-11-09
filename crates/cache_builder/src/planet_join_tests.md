# planet_join_tests.rs

Tests cover repeated way groups sharing one node decode, exact road coordinates,
missing IDs without fallback, a request window splitting within a node block,
multi-level external merges, checksum and truncation failures, interrupted
collection, uncommitted join output tails, mismatched settings/resume offsets,
and interrupted cell emission resuming from its durable group boundary.
Existing compact-versus-flat integration tests compare emitted cell bytes.

The ignored `benchmark_operator_sequential_join` requires ONEKEE_BENCH_SOURCE,
ONEKEE_BENCH_INDEX (a disposable copy), and ONEKEE_BENCH_OFFSET. It selects the
first 128 ways from four source blobs and repeats those dependency sets across
16 groups. The shipped shared-per-batch lookup and the full join preparation/load path run in
old/new/new/old order with exact coordinate comparison. Source access is read
only; scratch is under /tmp. This is a bounded repeated-dependency benchmark,
not a full-planet throughput measurement or guaranteed speedup.

Additional synthetic coverage spans 270 node blocks across three partitions and
multiple 16-block decode windows; all four output groups match, and exactly 270
original blocks are decoded. A repeated resolve performs zero original reads.

2026-11-09 operator sample (active builder left running): old/new/new/old times
23.845 / 8.599 / 8.588 / 23.165 seconds. Old: 2,696 reads, 4,909,162,932 bytes;
join: 649 reads, 1,182,609,238 bytes. Request and coordinate spools were each
989,952 bytes; combined process peak RSS 488,708 KiB. All coordinates matched.
The repeated sample models reuse across batches; cache warmth, concurrent I/O,
output cell writing, elevation and admin assembly limit extrapolation.

Fault injection now covers a damaged temporary sort with an uncommitted coordinate
tail, successful regeneration, a persistent fault stopping after exactly one retry,
and committed request corruption never being treated as disposable scratch.
An internal merge test rejects out-of-order records with a valid checksum.
The ignored `audit_operator_join_partition_sort` takes ONEKEE_JOIN_AUDIT_ROOT,
ONEKEE_BENCH_SOURCE and a disposable ONEKEE_BENCH_INDEX. It rebuilds the next saved
partition exclusively in /tmp, verifies hashes, all node/group ranges and sort
order, and confirms zero node-payload reads. Partition 232 passed all 22,966,241
records on 2026-11-09; existing build files remained unchanged.
