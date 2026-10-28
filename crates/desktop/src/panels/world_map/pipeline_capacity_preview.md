# pipeline_capacity_preview.rs

Opt-in GPU regression and visual QA using a real pipeline archive. Reads
ONEKEE_PIPELINE_ARCHIVE, prepares product/capacity batches, and checks that
geometry and picking owners remain identical. Renders both modes through the
real staged globe callback into a private texture; changing the mode must alter
pixels and returning to product must restore the map image exactly.

Includes the actual capacity legend and map caption. Set
ONEKEE_CAPACITY_PREVIEW to save the capacity frame as a PNG. No application window
or source-archive writes. Requires a working GPU and is ignored by default.

The preview submits the normal residency callback every frame to reset the shared upload budget. Optional diagnostic PNGs include both product frames for comparison.
