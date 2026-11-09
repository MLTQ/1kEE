# atlas_retention_gpu_tests.rs

Opt-in actual-device regression: upload terrain to both Atlas mode slots, switch
between them and hide/show without changing generation, and assert zero extra
upload bytes and retained render-target keys. Explicit invalidation must release
both slots and require a new upload. Selects ONEKEE_GPU when set, uses only
synthetic data and private offscreen targets, and waits for submitted GPU work.

Run `ONEKEE_GPU='RTX 4090' cargo test --release -p one-thousand-electric-eye-desktop atlas_gpu_revisit -- --ignored --nocapture --test-threads=1`.
