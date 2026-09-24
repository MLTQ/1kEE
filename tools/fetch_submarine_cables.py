#!/usr/bin/env python3
"""Refresh the bundled submarine cable snapshot from TeleGeography.

The desktop app ships a copy of the cable routes and landing points in
`crates/desktop/src/submarine_cables/` and compiles it in with `include_str!`,
so the layer works offline, instantly, and without touching the network.
Cables change a handful of times a year; rerun this when you want new ones.

Usage:

    python3 tools/fetch_submarine_cables.py

The snapshot is trimmed to what the renderer uses — each cable's name and
colour, each landing point's name — and coordinates are rounded to 4 decimal
places (about 11 m), which is far below anything visible on the globe.

Data: TeleGeography Submarine Cable Map, CC BY-NC-SA 3.0. See the LICENSE file
written alongside the snapshot.
"""

from __future__ import annotations

import json
import sys
import urllib.request
from pathlib import Path

CABLE_URL = "https://www.submarinecablemap.com/api/v3/cable/cable-geo.json"
LANDING_URL = "https://www.submarinecablemap.com/api/v3/landing-point/landing-point-geo.json"

REPO_ROOT = Path(__file__).resolve().parent.parent
OUT_DIR = REPO_ROOT / "crates" / "desktop" / "src" / "submarine_cables"

# A truncated or error-page response must never replace a good snapshot.
# The live feeds carry ~730 cables and ~1,925 landing points.
MIN_CABLES = 500
MIN_LANDINGS = 1000

DECIMALS = 4

USER_AGENT = "1kEE snapshot tool (+https://github.com/MLTQ/1kEE)"

LICENSE_TEXT = """\
Submarine cable routes and landing points in this directory are a snapshot of
the TeleGeography Submarine Cable Map (https://www.submarinecablemap.com/).

Copyright TeleGeography. Licensed under Creative Commons
Attribution-NonCommercial-ShareAlike 3.0 Unported (CC BY-NC-SA 3.0):
https://creativecommons.org/licenses/by-nc-sa/3.0/

This licence covers the data files here only. It is separate from the licence
of the 1kEE source code, and it does not permit commercial use of this data.

Regenerate with: python3 tools/fetch_submarine_cables.py
"""


def fetch(url: str) -> dict:
    print(f"fetching {url}", file=sys.stderr)
    request = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    with urllib.request.urlopen(request, timeout=120) as response:
        return json.load(response)


def round_coords(value):
    """Round every coordinate in a (possibly nested) GeoJSON coordinate array."""
    if isinstance(value, (int, float)):
        return round(value, DECIMALS)
    return [round_coords(item) for item in value]


def trim(collection: dict, keep: tuple[str, ...]) -> dict:
    features = []
    for feature in collection.get("features", []):
        geometry = feature.get("geometry")
        if not geometry or "coordinates" not in geometry:
            continue
        properties = feature.get("properties") or {}
        features.append(
            {
                "type": "Feature",
                "properties": {key: properties[key] for key in keep if key in properties},
                "geometry": {
                    "type": geometry["type"],
                    "coordinates": round_coords(geometry["coordinates"]),
                },
            }
        )
    return {"type": "FeatureCollection", "features": features}


def write(path: Path, collection: dict) -> None:
    # Compact separators: the file is compiled into the binary, not read by hand.
    path.write_text(json.dumps(collection, separators=(",", ":"), ensure_ascii=False) + "\n")
    print(f"wrote {path} ({len(collection['features'])} features, "
          f"{path.stat().st_size / 1024:.0f} KiB)", file=sys.stderr)


def main() -> int:
    cables = trim(fetch(CABLE_URL), keep=("name", "color"))
    landings = trim(fetch(LANDING_URL), keep=("name",))

    if len(cables["features"]) < MIN_CABLES or len(landings["features"]) < MIN_LANDINGS:
        print(
            f"error: got {len(cables['features'])} cables and "
            f"{len(landings['features'])} landings; refusing to overwrite the snapshot",
            file=sys.stderr,
        )
        return 1

    OUT_DIR.mkdir(parents=True, exist_ok=True)
    write(OUT_DIR / "cables.geojson", cables)
    write(OUT_DIR / "landings.geojson", landings)
    (OUT_DIR / "LICENSE").write_text(LICENSE_TEXT)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
