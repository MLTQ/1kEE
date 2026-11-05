# catalog.rs

Reads campus records embedded as JSON string literals in the public buildout
map's script assets. The decoder handles JavaScript quoting without executing
publisher code. Asset discovery follows same-origin Next.js static paths in
the current page, so content hashes may rotate.

A catalog must have unique IDs, in-range coordinates and finite, nonnegative
reported capacities. Missing MW stays unknown. Precision, project stage and
the publisher's verification date travel with each record.

Rules keep jurisdiction, legal status, severity, dates and optional published
coordinates. News retains original dates, citations and related entity IDs.
All three tables must validate together before replacing the previous snapshot.
