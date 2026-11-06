# recap_regions.rs

Defines eight exclusive geographic broadcast regions and their globe framing
centers and individual zoom levels. `RecapRegion::ALL` gives the stable recap
rotation; `at` assigns every valid lat/lon to one region and rejects invalid positions.

The coarse seams follow Panama, the Mediterranean/Red Sea, Urals/Caucasus and
western Pacific. Oceans belong to surrounding regions; these are display
partitions, not precise coastlines, administrative borders or political claims.
Oceania spans the date line. Antarctica covers positions below 60°S.

`follow_recap.rs` filters historical events by this single assignment, avoiding
overlapping rectangle recaps or omitted offshore events. Geographic fixtures
and a global position grid are covered in `follow_recap_tests.rs`.

Middle East is assigned before Africa/Europe/Asia and includes Egypt, Turkey,
the Levant, the Arabian Peninsula and Iran. Its coarse polygon leaves Greece,
the Caucasus capitals, Afghanistan, Pakistan and the Horn of Africa in their
neighboring regions. As with the continental seams, it is a broadcast grouping.

`zoom` tightens every region beyond the former shared 1.15 overview. Large Asia
uses 1.5; smaller Europe uses 2.0 and Middle East 2.2. The Middle East camera
center sits over Arabia (25°N, 45°E) to keep southern reports above the chyron.
The stable eight-stop order puts Middle East between Africa and Asia.
