# atlas_surface_forest.rs

## Purpose
Stage R8 forest coverage alongside an exact Atlas source without uploading
height/color again or exposing a partially uploaded mask.

## Contracts
Starts with a tiny empty binding. A new immutable mask replaces the texture and
uploads in <=512 KiB row stripes within the surface's shared 8 MiB frame budget.
`ready` requires matching mask identity and all rows. Source retirement drops
both CPU mask ownership and its texture. Visibility remains a material uniform.
