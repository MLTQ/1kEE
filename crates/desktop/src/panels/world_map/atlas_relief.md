# atlas_relief.rs

## Purpose
Load measured Atlas height fields and submit actual 3D terrain in both views.
Native data, pigmentation and errors are prepared asynchronously; painting never
opens files or invokes GDAL. Local view has no flat-image ground-plane fallback.

## Components
- `load_grid` provides the coarse global height field for the globe overview.
- `detail_texture` retains native metre heights as `atlas_surface::Raster`, not
  just a rendered image. Root/region/texture-limit/epoch keys reject stale work.
  Each raster owns the geographic bounds used for projection and publication.
- `draw_globe` submits overview and native height fields to the displaced sphere
  renderer, preserving their own geographic bounds during refinement.
- `draw_local` submits the native height field using the exact contour camera
  projection, including layer spread. Only real terrain is drawn in Local view.
- `paint_detail_notice` distinguishes source refinement, staged 3D preparation
  and actual source/tool errors. GPU publication IDs keep readiness honest.

## Contracts
Contour/coastline/bathymetry controls become surface isolines in Atlas. They use
exactly the same geometry and depth buffer as the colored terrain; other themes
retain their existing vector pipelines. Heights and colors persist together until
GPU staging completes; late status notices cannot override a new request/epoch.
Native GDAL discovery remains independent of the terminal PATH. Runtime tests
exercise source selection, staging, real drawing and contour toggles.

Height fields now retain unlit pigments. Both scene controllers pass saved art-direction controls and the app clock into the surface callback; light changes never invalidate source requests.

Local Trees requests exact-height-field coverage through `atlas_forest`; its material is applied by the actual terrain shader. Coverage never changes source elevations or light controls.

Hiding Local Trees suspends pending coverage work without discarding a reusable completed mask.

The global overview first reads the Overview tier in atlas_heights_v1.sqlite.
Existing GEBCO previews remain the fallback for incomplete/missing caches.
Detailed windows use the corresponding cache-first path in atlas_source.rs.

Interactive Atlas retains prepared heights and pigments within the configurable
ONEKEE_ATLAS_RAM_MIB LRU byte budget (1 GiB normally, 8 GiB in the 4090 profile).
There is no entry-count limit. Exact and compatible coverage revisits share the
original raster; both scenes can use it. A cache hit installs the ready snapshot
immediately, bypassing an unrelated pending load without releasing its worker gate. Explicit clear/epoch changes reject those in-memory entries. Pending
and current geographic requests are stabilized while their padded terrain still
contains the actual visible viewport, subject to detail-tier and zoom checks.
This prevents small pans from repeatedly discarding useful in-flight work.

Complete native windows are queued for background persistence after pigment
preparation. The rendering worker never waits for compression or cache writes.

With ONEKEE_ATLAS_KEEP_GLOBE, Globe uses a separate non-evicting prepared pool;
Local continues using RECENT's bounded LRU. The request's cache generation is
captured before its worker is spawned. Clear releases both pools and increments
their generations; old workers cannot read/publish into the cleared pool, even
if they start after the clear. Completed regions log retained count/RAM bytes.
