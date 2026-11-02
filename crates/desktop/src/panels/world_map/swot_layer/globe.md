# globe.rs

## Purpose
Draw the prepared gravity overview on the front-facing Earth sphere.

## Contracts
Uses the existing unit-sphere globe projection and a north-up global
UV map. Two-degree cells cover exactly −180…180 / −90…90; cells touching the
hidden surface are omitted using the perspective horizon z ≥ 1/camera distance,
rather than hemisphere culling. A test pins the projected sphere bounds and
global UV limits. A thread-local mesh cache includes texture ID
and every camera/layout input. Stationary frames reuse the same immutable mesh.
`clear` releases the mesh when visibility or terrain caches reset.
