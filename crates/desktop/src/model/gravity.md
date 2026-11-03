# gravity.rs

## Purpose
Hold the optional session-only gravity-anomaly selection independently of SWOT
VGG, terrain and elevation. The bundled JSON is also the importer's source list.

## Components
- `GravityKind` names the three physical quantities and provides concise help and a quantity-specific color range.
- `gravity_maps` decodes the small bundled catalog once without filesystem I/O.
- `GravityState` starts hidden, selects one product, and selects a valid default
  region when changing quantity. Out-of-range indices cannot corrupt selection.

## Contracts
The catalog must contain unique safe IDs and at least one map for each kind.
Products retain individual coverage and scientific qualifications. The model
neither downloads files nor combines unlike quantities. Renderer and drawer
use the same selected product. Tests pin selection and catalog identity.
