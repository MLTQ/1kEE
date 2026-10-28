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

The Globe test also compares sampled interior color against Local within two
8-bit steps, catching opacity/color-space regressions beyond simple nonzero fill.
