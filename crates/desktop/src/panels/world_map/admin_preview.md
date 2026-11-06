# admin_preview.rs

Opt-in headless GPU regression exercising the real controls and admin paint path.
Uses clearly synthetic geometry in a temporary cache. Checks interior fill,
unshaded holes and exact map restoration after a shading toggle round trip.
Set ONEKEE_ADMIN_PREVIEW to export the shaded frame for visual inspection.

The Globe variant drives the production GPU fill/outline callbacks with the same
hole fixture, checks exact shade-toggle restoration, then rotates to the opposite
hemisphere and verifies no fill leaks through. ONEKEE_ADMIN_GLOBE_PREVIEW exports
its shaded image. Both tests use an explicit source override and normalized
contour width, avoiding user settings writes from legacy slider rounding.

The Globe test compares sampled interior color against Local within two
8-bit steps under both Sodium and Topo. The same state polygon must change from
amber to cyan when using the other theme. The exported final frame uses Topo.
Both paths also verify that master opacity fades outlines and fills at 50%,
removes all geometry at 0%, and restores the exact original frame at 100%.

`admin_real_cache_renders_in_both_views` reads ONEKEE_ADMIN_REAL_CACHE and renders
published PBF relation 9407 (Andorra) without modifying that cache. It requires
complete topology, visible outlines, interior shading, an untouched exterior,
shade-toggle restoration and Globe backside occlusion. The same PNG environment
variables export both views; this test is opt-in because real data and a GPU are
external prerequisites. The Local projection is a fixed geographic QA viewport;
the outline/fill renderer and Globe callbacks are the production implementations.

GPU previews consume the LOD-backed loader: world detail for the synthetic hole
and island fixture, and a fine spatial request around Andorra for real-data QA.

The shared admin_test_screen target also drives admin_handoff_tests, which compares
every frame of a two-stream replacement against complete rendered reference images.
