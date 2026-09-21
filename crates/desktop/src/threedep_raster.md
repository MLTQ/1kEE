# threedep_raster.rs

## Purpose
Streams hosted terrain rasters to disk with bounded memory. Twenty-five
downloads no longer require twenty-five complete raster byte vectors in RAM.

## Components
- `save`: validates the existing TIFF prefix/size contract, writes through a
  64 KiB transfer buffer, reports actual bytes, checks Content-Length when
  present, and atomically renames a unique partial file on success.
- `Partial`: removes incomplete downloads on errors or unwinding. Existing
  destination files are preserved until a complete replacement is available.

## Contracts
| Dependent | Expects | Breaking changes |
|---|---|---|
| `threedep.rs` | Success only after validated bytes are published | Reporting EOF/error/partial data as ready |
| Build progress | Actual increasing byte counts; unknown length remains unknown | Synthetic progress |
| Memory/storage | One 64 KiB transfer buffer; maximum 64 MiB per response | Buffering whole responses or unlimited error bodies |

## Notes
The 64 MiB ceiling exceeds the existing 2400x2400 Float32 request size. These
files are disposable inputs; flush does not add a durable fsync per download.
Tests cover exact bytes, lengths, interrupted reads, cleanup, and preservation
of an existing destination on failure. Full TIFF validity is still checked by GDAL.
