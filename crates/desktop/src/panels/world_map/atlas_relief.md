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
