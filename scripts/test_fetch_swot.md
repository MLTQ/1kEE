# test_fetch_swot.py

Tests the actual GDAL conversion using a tiny raw Float32/VRT fixture. Verifies
physical Eötvös ×10 quantization, north-up registration, retained zero and other
finite values, NaN missing cells, polar caps, area-average overview and provenance.
Also rejects overwriting an existing prepared dataset, packed units and shifted
source grids. Requires GDAL for the conversion test; no network or large data.

Run `python3 -m unittest discover -s scripts -p 'test_fetch_swot.py'`.
