#!/usr/bin/env python3
"""Build a small capacity lookup from the pinned GEM raw pipeline snapshots."""
import argparse
import hashlib
import json
import pathlib

from fetch_pipelines import CAPACITY_RELEASES, SOURCES, capacity


def source_snapshot(source, path):
    records = {}
    unit = None
    for feature in json.loads(path.read_text())["features"]:
        if (feature.get("geometry") or {}).get("type") not in {"LineString", "MultiLineString"}:
            continue
        properties = feature.get("properties") or {}
        report = capacity(source, properties)
        if report is None:
            continue
        key = properties.get("project-id")
        if not isinstance(key, str) or not key or key in records:
            raise ValueError(f"Missing or duplicate project ID in {source}: {key}")
        if unit is not None and unit != report["unit"]:
            raise ValueError(f"Mixed capacity units in {source}")
        unit = report["unit"]
        records[key] = report["value"]
    if not records:
        raise ValueError(f"No reported capacities in {source}")
    with path.open("rb") as stream:
        digest = hashlib.file_digest(stream, "sha256").hexdigest()
    return dict(id=source, url=SOURCES[source][0], file=source + ".geojson",
                sha256=digest, unit=unit, source_release=CAPACITY_RELEASES[source],
                capacities=records)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--gas", type=pathlib.Path, required=True)
    parser.add_argument("--oil", type=pathlib.Path, required=True)
    parser.add_argument("--out", type=pathlib.Path, required=True)
    args = parser.parse_args()
    data = dict(version=1, attribution="Global Energy Monitor, adapted capacity metadata",
                license="CC BY 4.0", license_url="https://globalenergymonitor.org/creative-commons-license",
                sources=[source_snapshot("gem-gas", args.gas), source_snapshot("gem-oil", args.oil)])
    with args.out.open("x") as stream:
        json.dump(data, stream, sort_keys=True, indent=2, allow_nan=False)
        stream.write("\n")


if __name__ == "__main__":
    main()
