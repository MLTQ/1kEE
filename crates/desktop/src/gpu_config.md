# gpu_config.rs

Launch-time GPU adapter selection and bounded terrain budgets. `ONEKEE_GPU`
selects a case-insensitive adapter-name substring among surface-compatible devices,
with native backends preferred over GL. Unmatched explicit requests fail visibly.
Unset requests retain eframe selection. Device creation logs the actual adapter.

`Settings` caches validated memory/upload environment values in `OnceLock`; defaults
include separate Atlas prepared RAM and compressed disk budgets. Residency and both upload paths consume the same values.
Invalid, zero, out-of-range and overflowing values fall back with a diagnostic.
Tests validate boundaries without mutating process-global environment variables.
See [GPU tuning](../../../docs/gpu-performance.md) for ranges and limitations.

Both launch profiles retain offscreen Globe textures within `ONEKEE_ATLAS_VRAM_MIB`.
Set `ONEKEE_ATLAS_KEEP_GLOBE=0` to release them immediately.
`ONEKEE_ATLAS_PIN_MIB` limits verified regional relief files on disk.
