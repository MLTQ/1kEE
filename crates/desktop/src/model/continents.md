# continents.rs

Defines seven exclusive geographic broadcast regions and their globe framing
centers. `Continent::ALL` gives the stable recap rotation; `at` assigns every
valid lat/lon to one region and rejects invalid positions.

The coarse seams follow Panama, the Mediterranean/Red Sea, Urals/Caucasus and
western Pacific. Oceans belong to surrounding regions; these are display
partitions, not precise coastlines, administrative borders or political claims.
Oceania spans the date line. Antarctica covers positions below 60°S.

`follow_recap.rs` filters historical events by this single assignment, avoiding
overlapping rectangle recaps or omitted offshore events. Geographic fixtures
and a global position grid are covered in `follow_recap_tests.rs`.
