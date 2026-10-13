# hillshade_source_tests.rs

## Purpose
Verify the DNR wire contract and resource/coverage limits without network access.
An opt-in smoke test checks both actual Seattle hillshade products.

## Contracts
The offline tests pin longitude-first bounds, explicit layer selection, PNG alpha,
and rejection of invalid, excessive or geographically irrelevant requests.
The ignored live test downloads two 256×256 images, verifies visible varied
relief and different bare-earth/surface pixels, and saves no survey data.

Run `cargo test -p one-thousand-electric-eye-desktop hillshade` for offline checks;
add `-- --ignored --nocapture` for the live test with network access and detailed
terrain streaming enabled.
