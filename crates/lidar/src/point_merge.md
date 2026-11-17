# Original point-cloud union

Discovers up to 64 supported candidates from a bounded spatial catalog query.
Actual footprint masks prioritize center coverage, then previously unrepresented
parts of the radius. Masks guide acquisition order; actual source points are
still clipped with the exact WGS84 radial test.

A first pass reserves half the total point allowance across candidates. A
second pass resumes unfinished sources for detail. EPT node record offsets and
bulk-LAS ordinals avoid replaying accepted observations, including when a quota
ends in the middle of a compressed node. Points from different acquisitions
retain their original XYZ, attributes and source identity; they are neither
averaged nor spatially rounded to remove real repeated observations.

One StreamBudget governs all passes: points, decoded records, file/node reads,
network bytes and deadline. Source failures preserve already emitted batches
and let neighbors continue. Consumer cancellation stops the whole load.
Receipts distinguish complete reads, budgets, unavailable sources and conflicting
declared height references. Unknown height references are allowed together with
an explicit unverified flag, per the user's chosen policy. They are not aligned
to one another or to a DEM. Known conflicting EPSG vertical references require
single-source inspection.
