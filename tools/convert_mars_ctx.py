#!/usr/bin/env python3
"""
convert_mars_ctx.py — Compress CTX DEMs and DRGs in-place and delete junk files.

Typical run:
    python3 tools/convert_mars_ctx.py /Volumes/Hilbert/Data/Mars/mars_data

What it does per pair directory
────────────────────────────────
  CONVERT  *-DEM-geoid-adj.tif  →  Float32 → Int16, DEFLATE COG, Mars longlat
  CONVERT  *-DRG.tif            →  JPEG COG, Mars longlat
  DELETE   *-DEM.tif              (ellipsoid-relative, redundant)
           *-DEM-geoid-hs.tif     (pre-baked hillshade)
           *-DEM-geoid-hs.jpeg    (pre-baked hillshade, JPEG)
           *-IntersectionErr.tif  (stereo QC metric)
           *-FINAL_geodiff-diff.csv
           provenance.txt
           qa_metrics.txt

Each conversion writes to a sibling .tmp.tif first, then atomically renames it,
so an interrupted run never corrupts the original.  A hidden marker file
(.dem_done / .drg_done) is written on success so re-runs skip finished pairs.

Estimated results (based on 44,341 pairs at ~39 MB/pair total):
  Delete junk    → free ~16 MB/pair  → ~700 GB freed immediately
  Compress DEM   → ~3–4× smaller    → ~200 GB saved
  Compress DRG   → ~2–3× smaller    → ~250 GB saved
  Net total      → ~1.7 TB → ~200–300 GB  (roughly 6–8× reduction)

Options
───────
  --workers N     Parallel workers (default: 4; use 2 for a spinning HDD)
  --dem-only      Skip DRG conversion (if you only need elevation)
  --drg-only      Skip DEM conversion
  --delete-only   Delete junk files without converting anything
  --dry-run       Print what would happen; change nothing
  --jpeg-quality  JPEG quality for DRGs (default: 88; range 60–95)
"""

import argparse
import os
import shutil
import subprocess
import sys
import time
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path

MARS_LONGLAT_SRS = "+proj=longlat +R=3396190 +no_defs"
DEM_NODATA = -32767

# Files deleted unconditionally — QC artifacts and redundant products.
DELETE_SUFFIXES = [
    "-DEM.tif",
    "-DEM-geoid-hs.tif",
    "-DEM-geoid-hs.jpeg",
    "-IntersectionErr.tif",
    "-FINAL_geodiff-diff.csv",
]
DELETE_NAMES = ["provenance.txt", "qa_metrics.txt"]


# ── GDAL tool resolution ──────────────────────────────────────────────────────

def find_gdal_tool(name: str) -> str:
    """Return the path to a GDAL command-line tool."""
    search = [
        name,
        f"/opt/homebrew/bin/{name}",   # macOS arm64 Homebrew
        f"/usr/local/bin/{name}",      # macOS x86 Homebrew / Linux
        f"/usr/bin/{name}",
    ]
    for candidate in search:
        if shutil.which(candidate):
            return candidate
    # Fall back and let the OS raise a useful error if missing.
    return name


GDALWARP = find_gdal_tool("gdalwarp")


# ── Per-file conversion ───────────────────────────────────────────────────────

def _warp_to_cog(src: Path, dst: Path, extra_args: list[str], timeout: int) -> bool:
    """
    Run gdalwarp on `src` → a .tmp file → atomically rename to `dst`.
    Returns True on success.
    """
    tmp = dst.with_suffix(".tmp.tif")
    try:
        cmd = [
            GDALWARP, "-q", "-overwrite",
            "-t_srs", MARS_LONGLAT_SRS,
            "-r", "bilinear",
            "-of", "COG",
            "-co", "BIGTIFF=IF_SAFER",
            *extra_args,
            str(src), str(tmp),
        ]
        result = subprocess.run(cmd, capture_output=True, timeout=timeout)
        if result.returncode != 0:
            tmp.unlink(missing_ok=True)
            return False
        tmp.replace(dst)   # atomic on POSIX/APFS
        return True
    except Exception:
        tmp.unlink(missing_ok=True)
        return False


def convert_dem(src: Path, jpeg_quality: int) -> bool:
    """Float32 orthographic → Int16 DEFLATE COG in Mars longlat."""
    return _warp_to_cog(
        src, src,
        extra_args=[
            "-ot", "Int16",
            "-dstnodata", str(DEM_NODATA),
            "-co", "COMPRESS=DEFLATE",
            "-co", "PREDICTOR=2",   # delta filter — very effective for elevation
        ],
        timeout=300,
    )


def convert_drg(src: Path, jpeg_quality: int) -> bool:
    """Orthographic grayscale orthoimage → JPEG COG in Mars longlat."""
    return _warp_to_cog(
        src, src,
        extra_args=[
            "-co", "COMPRESS=JPEG",
            "-co", f"JPEG_QUALITY={jpeg_quality}",
        ],
        timeout=300,
    )


# ── Per-pair processing ───────────────────────────────────────────────────────

def process_pair(
    pair_dir: Path,
    do_dem: bool,
    do_drg: bool,
    do_delete: bool,
    jpeg_quality: int,
    dry_run: bool,
) -> dict:
    name = pair_dir.name
    msgs = []
    bytes_freed = 0

    # ── Delete junk files first — free space before temp files are written ────
    if do_delete:
        for suffix in DELETE_SUFFIXES:
            p = pair_dir / f"{name}{suffix}"
            if p.exists():
                sz = p.stat().st_size
                if not dry_run:
                    p.unlink()
                bytes_freed += sz
        for fname in DELETE_NAMES:
            p = pair_dir / fname
            if p.exists():
                sz = p.stat().st_size
                if not dry_run:
                    p.unlink()
                bytes_freed += sz

    # ── DEM conversion ────────────────────────────────────────────────────────
    if do_dem:
        dem = pair_dir / f"{name}-DEM-geoid-adj.tif"
        marker = pair_dir / ".dem_done"
        if dem.exists() and not marker.exists():
            before = dem.stat().st_size
            if dry_run:
                msgs.append(f"would convert DEM ({before/1e6:.1f} MB)")
            else:
                ok = convert_dem(dem, jpeg_quality)
                if ok:
                    after = dem.stat().st_size
                    marker.touch()
                    msgs.append(f"DEM {before/1e6:.1f}→{after/1e6:.1f} MB")
                    bytes_freed += before - after
                else:
                    msgs.append("DEM FAILED")

    # ── DRG conversion ────────────────────────────────────────────────────────
    if do_drg:
        drg = pair_dir / f"{name}-DRG.tif"
        marker = pair_dir / ".drg_done"
        if drg.exists() and not marker.exists():
            before = drg.stat().st_size
            if dry_run:
                msgs.append(f"would convert DRG ({before/1e6:.1f} MB)")
            else:
                ok = convert_drg(drg, jpeg_quality)
                if ok:
                    after = drg.stat().st_size
                    marker.touch()
                    msgs.append(f"DRG {before/1e6:.1f}→{after/1e6:.1f} MB")
                    bytes_freed += before - after
                else:
                    msgs.append("DRG FAILED")

    return {"name": name, "msgs": msgs, "freed": bytes_freed}


# ── Main ─────────────────────────────────────────────────────────────────────

def main():
    p = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    p.add_argument("mars_data", type=Path, help="Path to the mars_data/ directory")
    p.add_argument("--workers",      type=int,   default=4,  help="Parallel workers (default: 4)")
    p.add_argument("--jpeg-quality", type=int,   default=88, help="JPEG quality for DRGs (default: 88)")
    p.add_argument("--dem-only",     action="store_true", help="Convert DEMs only, skip DRGs")
    p.add_argument("--drg-only",     action="store_true", help="Convert DRGs only, skip DEMs")
    p.add_argument("--delete-only",  action="store_true", help="Delete junk files, skip conversion")
    p.add_argument("--dry-run",      action="store_true", help="Print what would happen; change nothing")
    args = p.parse_args()

    mars_data = args.mars_data.expanduser().resolve()
    if not mars_data.is_dir():
        print(f"Error: {mars_data} is not a directory", file=sys.stderr)
        sys.exit(1)

    do_dem    = not args.drg_only  and not args.delete_only
    do_drg    = not args.dem_only  and not args.delete_only
    do_delete = not args.dem_only  and not args.drg_only

    pair_dirs = sorted(d for d in mars_data.iterdir() if d.is_dir() and not d.name.startswith("."))
    total_pairs = len(pair_dirs)

    print(f"mars_data:  {mars_data}")
    print(f"Pair dirs:  {total_pairs:,}")
    print(f"Workers:    {args.workers}")
    print(f"Actions:    {'DEM ' if do_dem else ''}{'DRG ' if do_drg else ''}{'DELETE' if do_delete else ''}")
    if args.dry_run:
        print("DRY RUN — no files will be modified or deleted")
    print()

    done = 0
    failures = []
    total_freed = 0
    t0 = time.monotonic()

    with ThreadPoolExecutor(max_workers=args.workers) as pool:
        futures = {
            pool.submit(
                process_pair, d, do_dem, do_drg, do_delete, args.jpeg_quality, args.dry_run
            ): d
            for d in pair_dirs
        }
        for future in as_completed(futures):
            result = future.result()
            done += 1
            total_freed += result["freed"]

            if "FAILED" in " ".join(result["msgs"]):
                failures.append(result["name"])

            if done % 500 == 0 or done == total_pairs:
                elapsed = time.monotonic() - t0
                rate = done / elapsed if elapsed > 0 else 0
                eta = (total_pairs - done) / rate if rate > 0 else 0
                freed_gb = total_freed / 1e9
                print(
                    f"[{done:>6}/{total_pairs}]  "
                    f"{freed_gb:>6.1f} GB freed  |  "
                    f"{rate:.1f} pairs/s  |  "
                    f"ETA {eta/60:.0f} min"
                )

            # Always print failures immediately.
            for msg in result["msgs"]:
                if "FAILED" in msg:
                    print(f"  FAIL  {result['name']}: {msg}", file=sys.stderr)

    elapsed = time.monotonic() - t0
    print()
    print(f"Done in {elapsed/60:.1f} min")
    print(f"Space freed: {total_freed/1e9:.2f} GB")
    if failures:
        print(f"Failures ({len(failures)}):", file=sys.stderr)
        for f in failures:
            print(f"  {f}", file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    main()
