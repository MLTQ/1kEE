# contour_lifecycle_gpu_tests.rs

## Purpose
Verify actual globe GPU buffer residency through egui's callback preparation
order, without launching a window or accessing user data.

## Coverage
Sixty frames repeatedly replace two globe layers, hide one, then hide both.
Each layer uploads 8,192 real instances. After each submission the test asserts
the exact retained layer count and buffer bytes, including zero every third
frame. This checks cleanup runs after all prepares and cannot retain prior
generations. The test is opt-in because it requires a working GPU adapter.
