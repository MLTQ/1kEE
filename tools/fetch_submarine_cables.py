#!/usr/bin/env python3
"""Refresh the bundled submarine cable snapshot from TeleGeography.

The desktop app ships a copy of the cable routes and landing points in
`crates/desktop/src/submarine_cables/` and compiles it in with `include_str!`,
so the layer works offline, instantly, and without touching the network.
Cables change a handful of times a year; rerun this when you want new ones.

Usage:

    python3 tools/fetch_submarine_cables.py

The snapshot keeps what the app shows: each cable's route, colour, length,
owners and ready-for-service date, and each landing point's country and the
cables that land there. Landing details come from inverting the per-cable
records (~710 requests, one per cable system), which is far cheaper than fetching every landing point
(~1,900). Coordinates are rounded to 4 decimal places (about 11 m).

Data: TeleGeography Submarine Cable Map, CC BY-NC-SA 3.0. See the LICENSE file
written alongside the snapshot.
"""

from __future__ import annotations

import json
import sys
import time
import urllib.request
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

CABLE_URL = "https://www.submarinecablemap.com/api/v3/cable/cable-geo.json"
LANDING_URL = "https://www.submarinecablemap.com/api/v3/landing-point/landing-point-geo.json"
CABLE_DETAIL_URL = "https://www.submarinecablemap.com/api/v3/cable/{id}.json"

# Be gentle with a free public endpoint: a few requests in flight at once.
DETAIL_WORKERS = 4

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


def fetch(url: str, quiet: bool = False) -> dict:
    if not quiet:
        print(f"fetching {url}", file=sys.stderr)
    request = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    for attempt in range(3):
        try:
            with urllib.request.urlopen(request, timeout=120) as response:
                return json.load(response)
        except Exception:
            if attempt == 2:
                raise
            time.sleep(2 * (attempt + 1))
    raise AssertionError("unreachable")


def fetch_cable_details(cable_ids: list[str]) -> dict[str, dict]:
    """Fetch every cable's detail record, keyed by cable id."""
    print(f"fetching {len(cable_ids)} cable detail records", file=sys.stderr)

    def one(cable_id: str):
        return cable_id, fetch(CABLE_DETAIL_URL.format(id=cable_id), quiet=True)

    details = {}
    with ThreadPoolExecutor(max_workers=DETAIL_WORKERS) as pool:
        for done, (cable_id, record) in enumerate(pool.map(one, cable_ids), start=1):
            details[cable_id] = record
            if done % 100 == 0:
                print(f"  {done}/{len(cable_ids)}", file=sys.stderr)
    return details


def clean(value):
    """Normalise empty strings to None so the app sees one 'missing' shape."""
    if isinstance(value, str):
        value = value.strip()
        return value or None
    return value


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


def enrich(cables: dict, landings: dict, details: dict[str, dict]) -> None:
    """Attach cable details to cables, and the inverted cable list to landings."""
    landing_info: dict[str, dict] = {}
    for feature in cables["features"]:
        props = feature["properties"]
        record = details.get(props.get("id"), {})
        for key in ("length", "owners", "rfs", "url"):
            if clean(record.get(key)) is not None:
                props[key] = clean(record.get(key))
        if record.get("rfs_year"):
            props["rfs_year"] = record["rfs_year"]
        if record.get("is_planned"):
            props["is_planned"] = True
        for landing in record.get("landing_points") or []:
            info = landing_info.setdefault(landing["id"], {"country": None, "cables": []})
            info["country"] = info["country"] or clean(landing.get("country"))
            info["cables"].append(props["id"])

    for feature in landings["features"]:
        props = feature["properties"]
        info = landing_info.get(props.get("id"), {})
        if info.get("country"):
            props["country"] = info["country"]
        props["cables"] = sorted(set(info.get("cables", [])))


def main() -> int:
    raw_cables = fetch(CABLE_URL)
    cables = trim(raw_cables, keep=("id", "name", "color"))
    landings = trim(fetch(LANDING_URL), keep=("id", "name"))

    if len(cables["features"]) < MIN_CABLES or len(landings["features"]) < MIN_LANDINGS:
        print(
            f"error: got {len(cables['features'])} cables and "
            f"{len(landings['features'])} landings; refusing to overwrite the snapshot",
            file=sys.stderr,
        )
        return 1

    cable_ids = sorted({f["properties"]["id"] for f in cables["features"] if "id" in f["properties"]})
    details = fetch_cable_details(cable_ids)
    enrich(cables, landings, details)

    linked = sum(1 for f in landings["features"] if f["properties"]["cables"])
    print(f"{linked}/{len(landings['features'])} landing points linked to a cable", file=sys.stderr)

    OUT_DIR.mkdir(parents=True, exist_ok=True)
    write(OUT_DIR / "cables.geojson", cables)
    write(OUT_DIR / "landings.geojson", landings)
    (OUT_DIR / "LICENSE").write_text(LICENSE_TEXT)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
