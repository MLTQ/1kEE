# Target point coordinate transforms

Each point-loading thread reuses a private PROJ context and a normalized
horizontal transform for the current declared CRS. It converts up to 32,768
float64 XY pairs in place. Original source elevations and attributes stay with
their returns. Both paths disable PROJ network access. Cancellation is checked
between bounded batches. The configured GDAL executable remains the fallback
when the runtime library or requested operation is unavailable.
