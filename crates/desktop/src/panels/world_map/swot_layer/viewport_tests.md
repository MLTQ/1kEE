# viewport_tests.rs

Tests the real globe layout and inverse projector at overview/deep zoom, across
the dateline, at both poles and at a partially visible limb. Dense canvas samples
must lie inside each selected native window. Deep zoom must fit within the
texture limit without source decimation; high-DPI displays must refine earlier.
