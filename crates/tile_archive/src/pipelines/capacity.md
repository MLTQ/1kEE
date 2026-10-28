# capacity.rs

Defines optional source-reported pipeline capacity, with its original value,
unit and release. This is an asset attribute, never a flow observation or a
measurement of current utilization.

`Capacity::valid` requires a finite nonnegative value and nonempty unit/release.
Explicit source zero is preserved; missing values remain `None`. Renderers may
only compare capacities with compatible units. Unrecognized units can still be
shown on hover but must not enter a known-unit color scale.
