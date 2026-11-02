# globe.rs

## Purpose
Draw the gravity overview and automatically refine visible geography from
the native one-arcminute data as the globe magnifies.

## Contracts
Uses the existing unit-sphere globe projection and per-image geographic UV
maps. The overview stays visible while `viewport` chooses and asynchronously
loads a padded native window (up to 4096 pixels per edge, bounded by the GPU).
Overview cells cover exactly −180…180 / −90…90; detailed mesh cells adapt to
camera scale to bound curvature error. Cells touching the
hidden surface are omitted using the perspective horizon z ≥ 1/camera distance,
rather than hemisphere culling. A test pins the projected sphere bounds and
global UV limits. Independent thread-local mesh caches include texture ID
and every camera/layout input. Stationary frames reuse the same immutable mesh.
`clear` releases both meshes when visibility or terrain caches reset. Old
windows use their own geographic bounds while replacements load, so camera
movement never stretches old detail to a new location. Land values stay intact.
The regional mesh test verifies exact window UV edges and geographic projection
across the dateline independently of the global overview's UV coordinates.
