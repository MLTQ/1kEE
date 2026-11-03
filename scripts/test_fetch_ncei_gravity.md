# test_fetch_ncei_gravity.py

## Purpose
Verify scientific conversion behavior with deliberately asymmetric sample rows.

## Contracts
Fixtures pin physical scaling, south-to-north reversal, valid zero, scaled
integer and rounded Float32 missing sentinels, truncation rejection, geographic
pixel edges, prepared byte checksums and refusal to replace published output.
The catalog check catches unsafe/duplicate IDs and layout/size discrepancies.
Sibling-only GDAL VRT conversion is tested when GDAL is installed; other tests are offline.
