# atlas_source_tests.rs

Unit fixtures pin ocean-depth preservation, fine-source nodata fallback, bounded
SRTM filename selection and date-line wrapping. Opt-in GDAL tests read original
Europe GEBCO at 6240×3600 (15 arcsecond posts), the Alps at 3600×3600 (1 arcsecond
SRTM), and a synthetic georeferenced dateline fixture with known pixel values.
`ONEKEE_ATLAS_NATIVE_PREVIEW` optionally saves the real Europe relief PNG.
Original datasets and user settings are never modified.
