# atlas_retention_gpu_tests.rs

Opt-in actual-device regression: upload terrain to both Atlas mode slots, switch
between them and hide/show without changing generation, and assert zero extra
upload bytes and retained render-target keys. Explicit invalidation must release
both slots and require a new upload. Selects ONEKEE_GPU when set, uses only
synthetic data and private offscreen targets, and waits for submitted GPU work.

Run `ONEKEE_GPU='RTX 4090' cargo test --release -p one-thousand-electric-eye-desktop atlas_gpu_revisit -- --ignored --nocapture --test-threads=1`.

The geographic residency regression runs both bounded and pinned policies with
actual Globe callbacks and three different geographic windows. A-B-C-A must
upload zero extra bytes only when pinned. Local A-B-A must still re-upload under
both policies; abandoned partial uploads and explicit clear must free sources.

Verified on RTX 4090 / Vulkan, NVIDIA 615.78.08 (2026-11-09): the synthetic
A-B-C-A revisit uploaded 133,128 bytes with bounded residency and zero bytes
with Globe pinning. Both mode-switch and geographic retention regressions pass;
this proves upload reuse, not a whole-application frame-time benchmark.
