CREATE TABLE IF NOT EXISTS rasters (
 id INTEGER PRIMARY KEY AUTOINCREMENT, url TEXT UNIQUE NOT NULL,
 survey TEXT NOT NULL, date TEXT NOT NULL, path TEXT NOT NULL,
 source_path TEXT NOT NULL, source_bytes INTEGER NOT NULL, source_mtime_ns INTEGER NOT NULL,
 source_sha256 TEXT NOT NULL, raster_sha256 TEXT NOT NULL, raster_bytes INTEGER NOT NULL,
 source_crs TEXT NOT NULL, west REAL, south REAL, east REAL, north REAL,
 points INTEGER NOT NULL, valid_points INTEGER NOT NULL, valid_cells INTEGER NOT NULL,
 zoom INTEGER NOT NULL, created REAL NOT NULL
);
CREATE INDEX IF NOT EXISTS raster_location ON rasters(west,east);
CREATE INDEX IF NOT EXISTS raster_survey ON rasters(survey);
CREATE TABLE IF NOT EXISTS failures (url TEXT PRIMARY KEY, error TEXT NOT NULL, updated REAL NOT NULL);
