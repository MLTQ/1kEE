# globe_residency_tests.rs

## Purpose
Exercise long globe tours without a running app or mutable data drive.

## Coverage
Projection fixtures cover offscreen, backside, crossing, date-line and polar
patches. A 120-stop tour checks bounded source/metadata counts and actual release
through weak Arc references. Late-reader tests prove a completed old request
cannot repopulate terrain after the camera moves to another region.
An independent point projection checks that visible terrain at both radii stays
inside the conservative patch bounds, including an asymmetric viewport.
