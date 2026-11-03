Streams each source ring through all simplification tiers in one transaction.
Indexes overlapping endpoint-preserving pieces and whole rings in SQLite R-trees.
The finest tier stores offsets/stride and initial longitude instead of duplicating
source coordinates. A single invalid or incomplete ring marks its whole relation
outline-only. The caller owns the unpublished database until the build succeeds.

Malformed rings are counted for the drawer, and disable shading for that relation.
