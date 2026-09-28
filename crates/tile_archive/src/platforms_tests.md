# platforms_tests.rs

Tests archive publication/refusal, full metadata round-trip, namespace-preserving
world copies, default status/support filters, malformed positions/IDs/versions,
and staging cleanup. The ignored real-data check validates every installed point
and reconciles source/filter counts with provenance; set ONEKEE_PLATFORM_ARCHIVE.

World copying must preserve original payload bytes/coordinate precision, not reserialize renderer f32 values.
