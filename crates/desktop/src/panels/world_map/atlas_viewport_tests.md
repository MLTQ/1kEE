# atlas_viewport_tests.rs

Tests native sample preservation, GPU/memory limits, actual globe zoom and poles,
and deep rotated Local windows at both date-line sides. Fine Local selection
must not get stuck at the old one-degree window minimum; source UVs stay valid.

Includes wrapped-region containment and strict post-rounding image-budget checks.
