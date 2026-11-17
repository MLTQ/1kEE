# Local LiDAR coverage

Reads all spatially intersecting catalog survey footprints without depending
on prepared scans, a fused index or source-point acquisition. Teal outlines
share the Lidar sidebar toggle and also appear as temporary Target placement
guides. Polygon holes, multipolygons, lines and point footprints are preserved.

The query first selects row IDs using the covering extent index, then reads
geometry for matching records. This keeps wrapped/date-line predicates from
forcing an unrelated geometry-table scan. Loads occur in a background reader;
the previous snapshot remains drawable until replacement arrives. Bounds have
25% prefetch margins and catalog refresh/error backoff is bounded.

Snapshot limits are 2048 surveys, 250,000 vertices, 8 MiB per geometry and
32 MiB per request; a limit is reported in the layer status. Painting clips
segments to the visible chart and uses decoded terrain-cache heights when
available, otherwise the map datum plane. It starts no elevation requests,
file decodes or scan preparation. These are published coverage annotations,
not measured three-dimensional scan boundaries or a promise of data access.
