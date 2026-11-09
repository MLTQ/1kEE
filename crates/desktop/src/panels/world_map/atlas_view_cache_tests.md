# atlas_view_cache_tests.rs

Private fixtures test exact Float32/NaN-bit roundtrips, source identity changes,
checksums, truncated compressed payloads, regenerating an unreadable entry,
retention limits and refusal to modify an unrelated database. No original terrain
or user cache is changed.
