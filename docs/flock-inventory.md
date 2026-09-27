# Flock position inventory

The desktop app loads `Flock/cameras.tsv` inside its configured Data Root.
For this machine the original download is:

`/Volumes/Hilbert/Data/Flock/cameras.tsv`

The adjacent `download.json` records the source URL, retrieval date, checksum,
byte size and inventory counts. Source: <https://flocksurveillance.org/data/cameras.tsv>.

In **Layers → Public Data**, **Flock cameras & devices** controls the independent
overlay. It defaults on and shows records with `active=1` and `status=inService`.
**Include planned / inactive records** reveals the remaining usable positions.
**Reload file** rereads a replacement TSV in the background; no app restart is
needed for later file updates. Loading the layer does not contact the source.

The 2026-09-27 download contains 335,701 rows (50,291,519 bytes): 335,689 usable
positions, including 213,814 active/in-service positions. Twelve `(0,0)`
placeholders are omitted from drawing but remain in the original file. The
server reported Last-Modified 2026-08-28. Distinct devices sharing coordinates
are retained. Some listed types are equipment rather than cameras.

This is a published inventory snapshot, not verified complete coverage or live
camera access. Original names, device types, status, features, dates and rotation
values remain intact in the TSV. Rotation is not displayed as a compass heading
without an established convention. The app keeps Flock and DeFlock/OSM data and
attribution separate while sharing their cached point-marker rendering.
