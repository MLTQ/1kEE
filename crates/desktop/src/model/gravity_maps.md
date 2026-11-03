# gravity_maps.json

## Purpose
Single curated catalog for the desktop selectors and NCEI importer. Contains
five regional terrain-corrected free-air grids, eight Bouguer grids and four
isostatic residual grids. No alternate-format copies or basement/roughness maps.

## Contracts
IDs are unique safe path components; kind/label/spacing/note describe the actual
product. Download bytes and SHA256 pin the binary layouts. Import fields specify
sample type, physical scale, missing sentinel, row order, pixel-edge transform,
source projection and output sampling. Never infer these from filenames alone.
Original source headers and notes remain beside raw downloads on the data drive.

## Source qualifications
- SEG combines onshore Bouguer and offshore free-air; the selector says so.
- ISORES has a conflicting row-order sentence and an imprecise missing flag in
  its old notes/header. The binary mask agrees with SEG in the header's
  south-to-north order; the actual missing physical value is 999999.99 mGal.
- SEG/ISORES header extents span whole pixels despite a center-cell label.
  Other selected products' extents span sample centers; transforms account for it.
- Projected legacy headers omit the horizontal datum/ellipsoid. Clarke 1866 is
  an explicit preparation assumption with no claimed datum transformation.
  This is recorded in every affected output manifest.
- NGS geographic coordinates are retained. Their documented NAD83/WGS84
  difference is far below the grid spacing. Alaska longitudes remain unwrapped.
- Grid spacing is not guaranteed independently measured feature resolution.
