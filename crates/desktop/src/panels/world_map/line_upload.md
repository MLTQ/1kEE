# line_upload.rs

## Purpose
Spread large line-buffer allocations over multiple frames without publishing
half a replacement tile/layer.

## Contracts
`Upload` pins the input Arc and advances through unchanged instance slices in
at most 512 KiB pieces. `Budget` admits 4 MiB per frame and stops scheduling
after 2 ms of submission work by default. `ONEKEE_UPLOAD_MIB` and
`ONEKEE_UPLOAD_US` configure these shared limits at process startup; see
[GPU tuning](../../../../../docs/gpu-performance.md). A single driver allocation cannot be preempted.
Globe callbacks share a budget; local callbacks share their frame's budget.
Only completed uploads are drawable; hidden pending buffers are released.
