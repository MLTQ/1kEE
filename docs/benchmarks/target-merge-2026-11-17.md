# Combined NYC / New Jersey Target validation

Verified the production source coordinator, native EPT readers and GPU point
renderer on 2026-11-17. The Target center is 40.72° N, 74.025° W, with a 3 km
radius across the Hudson. GPU: NVIDIA RTX 4090 / Vulkan / driver 615.78.08.
Machine-readable results: [receipt](target-merge-2026-11-17.json).

| Check | Result |
| --- | --- |
| Combined point budget | 1,200,000 original observations |
| NYC source | 1,000,000 retained points |
| New Jersey SdL5 2014 source | 200,000 retained points |
| ARRA Northeast source | Read; no returns within this circle |
| NYC occupied 100 m cells | 2,111 |
| Combined occupied 100 m cells | 2,923 |
| Decoded record visits | 3,168,493 |
| Node reads | 61 |
| Network transfer | 0 bytes; warm bounded source cache |
| Original-point loading | 3.87 seconds |
| Final status | Partial source density at combined point budget |

The 812 additional occupied cells establish that neighboring measurements
extend the sampled coverage of the displayed NYC cloud. Cell occupancy is not
a measure of continuous coverage or survey accuracy. The NYC contribution is
itself budget-limited; this is not an equal-point-count comparison of full
individual source clouds or a full-density regional performance benchmark.

Both sources omit a declared vertical reference. The combined view flags
unverified heights and keeps original source Z without offsets, unit guesses
or datum alignment. Each GPU batch retains its survey header and publisher
identity. Different acquisitions remain real repeated observations in overlap.
The test waits for every progressive point buffer to upload, then saves the
rendered result under `/tmp/1kee-target-merged-hudson.png`.

Automated regressions cover complementary footprints ahead of repeated central
coverage, a dense source not starving its neighbor, continuation after source
failure, shared resource limits, consumer cancellation, known vertical mismatch
rejection, unverified-reference labeling, and EPT/LAS cursor resume without
replaying points. Existing original-coordinate, classification, noise-mask,
radial-clipping, Local-return and GPU depth/resource-release checks pass.
