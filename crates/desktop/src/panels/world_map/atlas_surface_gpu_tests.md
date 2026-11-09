# atlas_surface_gpu_tests.rs

Opt-in native GPU checks prove geometry rather than compare baked relief colors:
identical solid pigments must gain a mountain silhouette when heights change;
contours change pixels exclusively on the same opaque surface; layer spread
changes surface and contours together; overlapping near/far terrain gives the
same image in either draw order through the private depth buffer.

An original-data Alps test reads native SRTM, constructs the same displaced
surface and optionally saves `ONEKEE_ATLAS_3D_PREVIEW`. Synthetic fixture export
uses `ONEKEE_ATLAS_3D_FIXTURE`. Both targets are private and sources read-only.

`render_styled` shares the real surface callback with the movable-light hardware
fixture in `atlas_light_gpu_tests.rs`; the original geometry tests use defaults.

`render_material` additionally accepts exact-source forest coverage for GPU material/visibility/light regression.
