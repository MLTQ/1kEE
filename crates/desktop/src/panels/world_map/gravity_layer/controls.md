# controls.rs

## Purpose
Present the anomaly toggle, physical quantity and region selection without I/O.

## Components
`controls` offers three correction families, each product's geographic region,
original grid spacing and source qualification. A quantity-specific mGal legend
uses the same color transfer as the image reader. No reload or citation links.

## Contracts
Hidden initially and disabled away from Earth. Selection/hide invalidates
image and mesh caches; status from a previous selection is not shown as the new
map's status. Availability comes from background cache outcomes only.
