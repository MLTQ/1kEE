# globe_contour_density.rs

## Purpose
Bound wide-view native contour drawing by omitting complete elevation planes,
while preserving every original vertex of every displayed contour.

## Contracts
One global elevation spacing applies to all visible tiles and zoom fallback.
Selection doubles a 100 m base until conservative visible batches fit a four
million segment target. Zero elevation always survives; the target is soft when
that plane alone exceeds it. A 2.8 million restore threshold prevents oscillation.
Zooming into fewer batches restores finer planes, ultimately every cached plane.
This limits the interactive preview. Stationary texture refinement restores every
plane; cache contents, uploads, contour coordinates and local view are unchanged.
Adjacent selected instance spans share a GPU draw; no line endpoints are joined.
