#!/usr/bin/env python3
"""Fetch public pipeline snapshots and normalize routes for the Rust packer."""
import argparse
import collections
import hashlib
import json
import pathlib
import subprocess
import tempfile
import urllib.parse

EIA = "https://arcgis.netl.doe.gov/server/rest/services/Hosted/EIA_pipeline_data/FeatureServer"
GEM = "https://publicgemdata.nyc3.cdn.digitaloceanspaces.com/interim_maps/"
SOURCES = {
    "gem-gas": (GEM + "ggit-lng_map_2025-11.geojson", "GEM GGIT November 2025 public map", "CC BY 4.0", "https://globalenergymonitor.org/projects/global-gas-infrastructure-tracker"),
    "gem-oil": (GEM + "goit_map_2026-06.1.geojson", "GEM GOIT June 2026 public map", "CC BY 4.0", "https://globalenergymonitor.org/projects/global-oil-infrastructure-tracker"),
    "bsee": ("https://www.data.bsee.gov/Mapping/Files/ppl_arcs.zip", "BSEE offshore pipelines", "US Government data", "https://www.data.bsee.gov/Main/Mapping.aspx"),
}
PRODUCTS = {0: "liquids", 1: "liquids", 2: "oil", 3: "gas"}
BSEE_GAS = {"BLKG", "GAS", "LIFT", "SPLY", "FLG", "BLGH", "INJ", "GASH", "NGER"}
BSEE_OIL = {"BLKO", "OIL", "BLOH", "OILH", "O/W"}
BSEE_MIXED = {"G/C", "G/O", "G/OH", "G/CH"}
HISTORICAL = {"ABN", "REM", "OUT", "A/C", "COMB", "CNCL", "RELQ", "R/A", "R/R", "R/C", "O/C"}


def fetch(url, path):
    """Preserve already-downloaded snapshots; atomically publish new files."""
    if path.exists():
        return
    part = path.with_suffix(path.suffix + ".part")
    try:
        subprocess.run(["curl", "--fail", "--location", "--silent", "--show-error", "--retry", "3", "--max-time", "300", "--output", str(part), url], check=True)
        part.rename(path)
    finally:
        part.unlink(missing_ok=True)


def query(url, params, path):
    fetch(url + "/query?" + urllib.parse.urlencode(params), path)
    data = json.loads(path.read_text())
    if "error" in data:
        raise ValueError(f"Server rejected {url}: {data['error']}")
    return data


def eia(root, layer):
    """Fetch by object ID, verifying every record, independent of page limits."""
    url = f"{EIA}/{layer}"
    ids = query(url, {"where": "1=1", "returnIdsOnly": "true", "f": "json"}, root / f"eia-{layer}-ids.json")
    expected = sorted(ids["objectIds"])
    field = ids["objectIdFieldName"]
    found = set()
    features = []
    for start in range(0, len(expected), 200):
        batch = expected[start:start + 200]
        data = query(url, {"objectIds": ",".join(map(str, batch)), "outFields": "*", "outSR": "4326", "f": "geojson"}, root / f"eia-{layer}-{start:06}.geojson")
        batch_ids = {f["properties"][field] for f in data["features"]}
        if batch_ids != set(batch) or found & batch_ids:
            raise ValueError(f"Incomplete or duplicate EIA page {layer}/{start}")
        found.update(batch_ids)
        features.extend(data["features"])
    if found != set(expected):
        raise ValueError("EIA feature count mismatch")
    return features


def info(source, p, row, product=None):
    base = dict(source=source, source_id=str(row), name="", operator="", owner="", product=product or "mixed", status="unknown", accuracy="Source route; accuracy unspecified", historical=False, planned=False)
    if source.startswith("eia-"):
        base.update(name=p.get("pipename") or "", operator=p.get("opername") or p.get("operator") or "", source_id=str(p.get("fid", row)))
    elif source == "bsee":
        code, status = p.get("PROD_CODE"), p.get("STATUS_COD", "unknown")
        product = "gas" if code in BSEE_GAS else "oil" if code in BSEE_OIL else "mixed" if code in BSEE_MIXED else "liquids" if code in {"NGL", "COND", "LGER"} else None
        if product is None:
            return None  # Water, power cables, umbilicals, chemicals are not fuel routes.
        base.update(source_id=str(p.get("SEGMENT_NU", row)), name=f"Offshore segment {p.get('SEGMENT_NU', row)}", operator=p.get("SDE_COMPAN") or "", product=product, status={"ACT": "active", "PABN": "proposed abandonment", "PREM": "proposed removal", "PROP": "proposed"}.get(status, status), historical=status in HISTORICAL, planned=status == "PROP")
    else:
        status = str(p.get("status") or "unknown").lower()
        fuel = str(p.get("fuel") or "").lower()
        base.update(source_id=str(p.get("project-id", row)), name=" · ".join(str(p[k]) for k in ("name", "unit-name") if p.get(k)), owner=p.get("owner") or "", product="gas" if source == "gem-gas" else "oil" if fuel == "oil" else "liquids", status=status, historical=status in {"retired", "cancelled", "idle", "idled", "mothballed", "shelved"}, planned=status in {"proposed", "construction", "under construction"}, accuracy="Mixed route accuracy; may be approximated between endpoints")
    return base


def normalize(source, features, stream, product=None):
    stats = collections.Counter()
    for row, feature in enumerate(features):
        geometry = feature.get("geometry") or {}
        kind = geometry.get("type")
        if kind not in ("LineString", "MultiLineString"):
            stats["non_route_records"] += 1
            continue
        metadata = info(source, feature.get("properties") or {}, row, product)
        if metadata is None:
            stats["non_fuel_records"] += 1
            continue
        stats["source_features"] += 1
        stats["historical" if metadata["historical"] else "planned" if metadata["planned"] else "current_or_unspecified"] += 1
        parts = [geometry["coordinates"]] if kind == "LineString" else geometry["coordinates"]
        for points in parts:
            if len(points) < 2:
                stats["empty_parts"] += 1
                continue
            if any(len(point) < 2 for point in points):
                raise ValueError(f"Invalid coordinate in {source} record {row}")
            stats["extra_ordinates_omitted"] += sum(len(point) > 2 for point in points)
            stream.write(json.dumps({"info": metadata, "points": [point[:2] for point in points]}, separators=(",", ":"), allow_nan=False) + "\n")
            stats["parts"] += 1
            stats["vertices"] += len(points)
    return dict(stats)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out-dir", type=pathlib.Path, required=True, help="New snapshot directory; raw downloads can be resumed")
    args = parser.parse_args()
    root = args.out_dir
    root.mkdir(parents=True, exist_ok=True)
    routes = root / "routes.jsonl"
    if routes.exists() or (root / "manifest.json").exists():
        raise SystemExit("Snapshot already normalized; use a new output directory")
    sources = []
    with tempfile.NamedTemporaryFile(mode="w", dir=root, prefix=".routes-", delete=False) as out:
        stage = pathlib.Path(out.name)
        try:
            for layer, product in PRODUCTS.items():
                source = f"eia-{layer}"
                features = eia(root, layer)
                stats = normalize(source, features, out, product)
                sources.append(dict(id=source, name=f"EIA {product} pipelines (DOE hosted)", url=f"{EIA}/{layer}", license="US Government data", stats=stats))
                print(source, stats, flush=True)
            for source, (url, title, license_name, homepage) in SOURCES.items():
                path = root / (source + (".zip" if source == "bsee" else ".geojson"))
                fetch(url, path)
                input_path = path
                if source == "bsee":
                    input_path = root / "bsee-wgs84.geojson"
                    if not input_path.exists():
                        subprocess.run(["ogr2ogr", "-f", "GeoJSON", "-t_srs", "EPSG:4326", "-ct_opt", "ALLOW_BALLPARK=NO", "-lco", "RFC7946=YES", str(input_path), "/vsizip/" + str(path.resolve())], check=True)
                with input_path.open() as f:
                    features = json.load(f)["features"]
                stats = normalize(source, features, out)
                sources.append(dict(id=source, name=title, url=url, homepage=homepage, license=license_name, stats=stats))
                print(source, stats, flush=True)
        except BaseException:
            stage.unlink(missing_ok=True)
            raise
    stage.rename(routes)
    import datetime
    files = []
    for path in sorted(root.iterdir()):
        if path.is_file() and not path.name.startswith("."):
            with path.open("rb") as f:
                digest = hashlib.file_digest(f, "sha256").hexdigest()
            files.append(dict(file=path.name, bytes=path.stat().st_size, sha256=digest))
    manifest = dict(version=1, fetched_utc=datetime.datetime.now(datetime.timezone.utc).isoformat(), sources=sources, files=files, notes=["GEM public map snapshots, not the full tracker release; missing routes remain missing.", "Sources overlap: separate source toggles preserve provenance; no name-based deduplication.", "BSEE NAD27 coordinates transformed to WGS84 with GDAL; ballpark transforms disallowed.", "Status/accuracy unspecified by the source remains unknown. Original fields are retained in raw downloads."])
    (root / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")


if __name__ == "__main__":
    main()
