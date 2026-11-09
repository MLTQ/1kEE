# atlas_light_gpu_tests.rs

An opt-in hardware fixture keeps the same source ID, heights, pigments and
camera while moving a low light across a measured ridge. Coverage must stay
identical; a flat valley 8 km away must enter/leave the cast shadow. Disabling
relief contrast removes all light-direction differences, proving no baked
shadows remain. Zero richness must produce grayscale terrain. Optional Sunlight
must also respond to a twelve-hour time offset on the same surface. Grayscale
allows one 8-bit rounding step across surface color channels; the test harness's
blue-black background is excluded from that color assertion.
