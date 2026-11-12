#!/usr/bin/env python3
"""Discover geolocated lidar holdings, export footprints and stream selected DTMs."""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import re
import sqlite3
import subprocess
import sys
import tempfile
import time
from urllib.error import HTTPError, URLError
from urllib.parse import quote, unquote, urlencode, urljoin, urlsplit
from urllib.request import Request, urlopen

OT = "https://portal.opentopography.org"
OT_CATALOG = OT + "/API/otCatalog?productFormat=PointCloud&outputFormat=json&include_federated=true&detail=true"
OT_STAC = OT + "/stac/raster_catalog.json"
EBA_LATEST = "https://zenodo.org/api/records/4968706/versions/latest"
EBA_ARCHIVES = "https://zenodo.org/search?q=" + quote('"Discrete airborne LiDAR transects collected by EBA"')
CMR = "https://cmr.earthdata.nasa.gov/search/granules.json?"
COLLECTIONS = {
    "nasa-brazil": "C2398128915-ORNL_CLOUD",
    "gliht-dtm": "C2763264722-LPCLOUD",
    "gliht-cloud": "C2763264735-LPCLOUD",
}
SWISS = "https://data.geo.admin.ch/api/stac/v1/collections/ch.swisstopo.swisssurface3d/items?limit=100"
LINZ = "https://nz-coastal.s3.ap-southeast-2.amazonaws.com/catalog.json"
IRELAND = ("https://gsi.geodata.gov.ie/server/rest/services/Marine/"
           "IE_GSI_MI_Marine_Download_Seabed_Survey_Leg_Data_IE_Waters_WGS84/MapServer/0")
MARINE = ("linz-coastal", "infomar-lidar")
DAY = 86400
MAX_METADATA = 100 * 1024 * 1024

SCHEMA = """
CREATE TABLE IF NOT EXISTS surveys (
    id TEXT PRIMARY KEY, source TEXT NOT NULL, name TEXT NOT NULL, product TEXT NOT NULL,
    date TEXT, page TEXT NOT NULL, asset TEXT, stac TEXT, geometry TEXT NOT NULL,
    west REAL, south REAL, east REAL, north REAL
);
CREATE INDEX IF NOT EXISTS surveys_extent ON surveys(west,east,south,north);
CREATE INDEX IF NOT EXISTS surveys_source ON surveys(source);
CREATE TABLE IF NOT EXISTS refreshes (source TEXT PRIMARY KEY, checked REAL, version TEXT);
CREATE TABLE IF NOT EXISTS resources (url TEXT PRIMARY KEY, etag TEXT, modified TEXT, body BLOB);
"""


def request(url, headers=None):
    req = Request(url, headers={"User-Agent": "1kEE-lidar-catalog/1", **(headers or {})})
    with urlopen(req, timeout=60) as response:
        body = response.read(MAX_METADATA + 1)
        if len(body) > MAX_METADATA:
            raise ValueError("Metadata response exceeds 100 MiB")
        return body, response.headers


def cached(db, url):
    row = db.execute("SELECT etag,modified,body FROM resources WHERE url=?", (url,)).fetchone()
    headers = {}
    if row and row[0]:
        headers["If-None-Match"] = row[0]
    if row and row[1]:
        headers["If-Modified-Since"] = row[1]
    try:
        body, response = request(url, headers)
    except HTTPError as exc:
        if exc.code == 304 and row:
            return row[2]
        raise
    db.execute("INSERT OR REPLACE INTO resources VALUES(?,?,?,?)",
               (url, response.get("ETag"), response.get("Last-Modified"), body))
    return body


def polygons(geometry):
    if geometry["type"] == "Polygon":
        return [geometry["coordinates"]]
    if geometry["type"] == "MultiPolygon":
        return geometry["coordinates"]
    raise ValueError("Expected polygon coverage")


def bounds(geometry):
    points = ([geometry["coordinates"]] if geometry["type"] == "Point" else
              [pt for poly in polygons(geometry) for pt in poly[0]])
    if not points or not all(-180 <= x <= 180 and -90 <= y <= 90 for x, y, *_ in points):
        raise ValueError("Invalid WGS84 footprint")
    return (min(p[0] for p in points), min(p[1] for p in points),
            max(p[0] for p in points), max(p[1] for p in points))


def rectangle(w, s, e, n):
    return [[[w, s], [e, s], [e, n], [w, n], [w, s]]]


def polygon_geometry(parts):
    if len(parts) == 1:
        return {"type": "Polygon", "coordinates": parts[0]}
    return {"type": "MultiPolygon", "coordinates": parts}


def record(key, name, product, date, page, geometry, asset=None, stac=None):
    return (key, name, product, date, page, asset, stac,
            json.dumps(geometry, separators=(",", ":")), *bounds(geometry))


def opentopography(data, catalog):
    rasters = {Path(urlsplit(l["href"]).path).name.removesuffix("_collection.json"): l["href"]
               for l in catalog["links"] if l["rel"] == "child"}
    # The provider sometimes publishes several footprint components under one DOI.
    datasets = {}
    for wrapped in data["Datasets"]:
        d = wrapped["Dataset"]
        ident = d["identifier"]
        key, provider = ident["value"], ident["propertyID"]
        name = d["name"]
        stac = None
        if provider == "USGS_3DEP_ID":
            page = OT + "/usgsDataset?" + urlencode({"dsid": key})
            product = "USGS 3DEP point cloud"
        elif provider == "NOAAID":
            page = OT + "/noaaDataset?" + urlencode({"noaaID": key})
            product = "NOAA coastal point cloud"
        elif provider == "opentopoID":
            # The point-cloud catalog also contains photogrammetry and sonar.
            labels = name + " " + (d.get("keywords") or "")
            evidence = labels + " " + (d.get("description") or "")
            laser = r"\blidar\b|laser scan|laser altimetr|light detection and ranging"
            other = r"photogrammetr|structure.from.motion|\bsfm\b|multibeam|sonar"
            if not re.search(laser, labels, re.I) and (
                    not re.search(laser, evidence, re.I) or re.search(other, evidence, re.I)):
                continue
            page = d["url"]
            product = "OpenTopography point cloud"
            stac = rasters.get(d.get("alternateName"))
        else:
            continue
        features = d["spatialCoverage"]["geo"]["geojson"]["features"]
        parts = [p for f in features for p in polygons(f["geometry"])]
        uid = provider + ":" + key
        if uid not in datasets:
            datasets[uid] = (name, product, d.get("temporalCoverage"), page, stac, [])
        rings = datasets[uid][-1]
        rings.extend(p for p in parts if p not in rings)
    for uid, (name, product, date, page, stac, parts) in datasets.items():
        yield record(uid, name, product, date, page, polygon_geometry(parts), stac=stac)


def eba_records(release, archive):
    body, _ = request(archive["links"]["self"])
    algorithm, digest = archive["checksum"].split(":", 1)
    if len(body) != archive["size"] or hashlib.new(algorithm, body).hexdigest() != digest:
        raise ValueError("Zenodo transect archive failed its size or checksum check")
    page = release["links"]["self_html"]
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp, "transects.zip")
        path.write_bytes(body)
        # Simplification is in the source projection (metres), before reprojection.
        result = subprocess.run(["ogr2ogr", "-f", "GeoJSON", "-t_srs", "EPSG:4326",
                                 "-select", "transect,datafile", "-simplify", "5",
                                 "-lco", "COORDINATE_PRECISION=6", "/vsistdout/",
                                 "/vsizip/" + path.as_posix()], check=True, stdout=subprocess.PIPE)
        for f in json.loads(result.stdout)["features"]:
            props = f["properties"]
            key = str(props["transect"])
            name = f"Brazilian Amazon EBA {key} · {props['datafile']}"
            yield record("EBA:" + key, name, "EBA transect / regional scan archives",
                         "2016–2018", page, f["geometry"], EBA_ARCHIVES)


def cmr_geometry(entry):
    parts = []
    for group in entry.get("polygons", []):
        for ring in group:
            numbers = list(map(float, ring.split()))
            if len(numbers) < 6 or len(numbers) % 2:
                raise ValueError("Invalid NASA polygon")
            coords = [[lon, lat] for lat, lon in zip(numbers[::2], numbers[1::2])]
            if coords[-1] != coords[0]:
                coords.append(coords[0])
            parts.append([coords])
    if not parts:
        for box in entry.get("boxes", []):
            s, w, n, e = map(float, box.split())
            parts.extend([rectangle(w, s, e, n)] if w <= e else
                         [rectangle(w, s, 180, n), rectangle(-180, s, e, n)])
    if not parts and len(entry.get("points", [])) == 1:
        lat, lon = map(float, entry["points"][0].split())
        return {"type": "Point", "coordinates": [lon, lat]}
    return polygon_geometry(parts)


def cmr_page(collection, size, headers=None, **query):
    url = CMR + urlencode({"collection_concept_id": collection, "page_size": size, **query})
    body, response = request(url, headers)
    if response.get("CMR-Timed-Out", "").lower() == "true":
        raise ValueError("NASA search timed out")
    return json.loads(body)["feed"]["entry"], response


def cmr_version(collection):
    entries, headers = cmr_page(collection, 1, sort_key="-revision_date")
    if not entries:
        raise ValueError("NASA collection has no granules")
    return f"{headers['CMR-Hits']}:{entries[0]['id']}:{entries[0]['updated']}"


def cmr_records(collection, expected):
    seen = set()
    cursor = None
    while len(seen) < expected:
        headers = {"CMR-Search-After": cursor} if cursor else {}
        entries, response = cmr_page(collection, 1000, headers)
        if not entries or int(response["CMR-Hits"]) != expected:
            raise ValueError("NASA catalog changed during pagination")
        for d in entries:
            if d["id"] in seen:
                raise ValueError("NASA returned a duplicate page or granule")
            seen.add(d["id"])
            for asset in d.get("links", []):
                url = asset.get("href", "")
                if not (url.startswith("https://") and asset.get("rel", "").endswith("/data#")
                        and not asset.get("inherited")):
                    continue
                name = urlsplit(url).path.rsplit("/", 1)[-1]
                if not name.lower().endswith((".laz", ".las", ".las.gz", ".tif", ".tiff")):
                    continue
                product = "NASA lidar raster" if name.lower().endswith((".tif", ".tiff")) else "NASA lidar point cloud"
                page = "https://search.earthdata.nasa.gov/search/granules?" + urlencode({"p": collection, "g": d["id"]})
                yield record(d["id"] + ":" + name, name, product, d.get("time_start"),
                             page, cmr_geometry(d), url)
        following = response.get("CMR-Search-After")
        if len(seen) < expected and (not following or following == cursor):
            raise ValueError("NASA omitted the next page cursor")
        cursor = following
    if len(seen) != expected:
        raise ValueError("Incomplete NASA catalog")


def stac_records(db, source, first):
    """Follow an external lidar collection's GET pagination, keeping only published scan assets."""
    url = first
    seen = set()
    while url:
        if url in seen or not url.startswith("https://"):
            raise ValueError("Invalid or repeated STAC next-page link")
        seen.add(url)
        page = json.loads(cached(db, url))
        if page.get("type") != "FeatureCollection":
            raise ValueError("Expected a STAC items FeatureCollection")
        for f in page["features"]:
            props = f["properties"]
            links = {l["rel"]: urljoin(url, l["href"]) for l in f["links"]}
            for key, asset in f["assets"].items():
                href = urljoin(url, asset["href"])
                if not href.startswith("https://"):
                    continue
                ext = urlsplit(href).path.lower()
                if ext.endswith((".las", ".laz", ".las.zip", ".laz.zip", ".las.gz")):
                    product = "Lidar point cloud"
                elif ext.endswith((".tif", ".tiff")) and "data" in asset.get("roles", []):
                    product = "Lidar raster"
                else:
                    continue
                label = props.get("title") or f["id"]
                yield record(f"{source}:{f['id']}:{key}", label, product,
                             props.get("datetime") or props.get("start_datetime"),
                             links.get("alternate") or links.get("self") or first, f["geometry"], href)
        next_link = next((l for l in page["links"] if l["rel"] == "next"), None)
        if next_link and next_link.get("method", "GET") != "GET":
            raise ValueError("Only GET pagination is supported for STAC items")
        url = urljoin(url, next_link["href"]) if next_link else None


def linz_document(db, parent, link):
    url = urljoin(parent, link["href"])
    checksum = link["file:checksum"]
    row = db.execute("SELECT body FROM resources WHERE url=? AND etag=?", (url, checksum)).fetchone()
    if row:
        return url, json.loads(row[0])
    body, _ = request(url)
    if "1220" + hashlib.sha256(body).hexdigest() != checksum:
        raise ValueError(f"LINZ metadata checksum mismatch: {url}")
    db.execute("INSERT OR REPLACE INTO resources VALUES(?,?,NULL,?)", (url, checksum, body))
    return url, json.loads(body)


def linz_coastal(db, root):
    for child in root["links"]:
        if child["rel"] != "child" or "/dem_1m/" not in child["href"]:
            continue
        collection_url, coll = linz_document(db, LINZ, child)
        if "lidar" not in coll["title"].lower():
            continue
        for link in coll["links"]:
            if link["rel"] != "item":
                continue
            _, item = linz_document(db, collection_url, link)
            item_url = urljoin(collection_url, link["href"])
            raster = item["assets"]["visual"]
            if "cloud-optimized" not in raster["type"]:
                raise ValueError(f"Expected a coastal COG at {item_url}")
            yield record("LINZ:" + item_url, coll["title"] + " · " + item["id"],
                         "LINZ topobathymetric lidar DEM (1 m)", item["properties"].get("start_datetime"),
                         "https://radiantearth.github.io/stac-browser/#/external/" + item_url,
                         item["geometry"], urljoin(item_url, raster["href"]))


def irish_query():
    # These are the airborne survey platforms and the publisher's lidar report directory.
    return IRELAND + "/query?" + urlencode({
        "where": "UPPER(SURVREPURL) LIKE '%/LIDAR/%' OR UPPER(VESSELNAME) LIKE 'LADS%' "
                 "OR UPPER(VESSELNAME) LIKE 'PELYDRYN HAWK EYE%' OR UPPER(VESSELNAME) = 'BLOMS GREATMANS BAY'",
        "outFields": "OBJECTID,GLOBALID,VESSELNAME,SURVEYYEAR,SURVREPURL,SR_URL,GRID_URL,XYZ_URL",
        "returnGeometry": "true", "outSR": 4326, "geometryPrecision": 6, "f": "geojson"})


def infomar(data):
    if data.get("exceededTransferLimit") or data.get("properties", {}).get("exceededTransferLimit"):
        raise ValueError("Incomplete Irish lidar inventory")
    for f in data["features"]:
        p = f["properties"]
        # Expose the publisher's existing render as well as its measured grid/XYZ when supplied.
        for key, product in (("SR_URL", "shaded relief"), ("GRID_URL", "depth grid"), ("XYZ_URL", "XYZ soundings")):
            asset = p.get(key) or ""
            if not asset.startswith("https://") or not asset.lower().endswith(".zip"):
                continue
            name = unquote(urlsplit(asset).path.split("/")[-1]).split("_5m")[0]
            page = p["SURVREPURL"] if (p.get("SURVREPURL") or "").startswith("https://") else IRELAND + "/" + str(p["OBJECTID"])
            yield record("INFOMAR:" + p["GLOBALID"] + ":" + key, "Ireland · " + name.replace("_", " "),
                         "INFOMAR bathymetric lidar " + product, str(p["SURVEYYEAR"]), page, f["geometry"], asset)


def refresh_source(db, source, force, collections, stac=None):
    stac = stac or {}
    with db:
        db.execute("BEGIN IMMEDIATE")
        row = db.execute("SELECT checked,version FROM refreshes WHERE source=?", (source,)).fetchone()
        interval = 7 * DAY if source in stac else DAY
        if row and not force and time.time() - row[0] < interval:
            return "checked within 7 days" if source in stac else "checked within 24h"
        if source == "opentopography":
            data, rasters = cached(db, OT_CATALOG), cached(db, OT_STAC)
            version = "lidar-v2:" + hashlib.sha256(data + rasters).hexdigest()
            records = lambda: opentopography(json.loads(data), json.loads(rasters))
        elif source in MARINE:
            body = cached(db, LINZ if source == "linz-coastal" else irish_query())
            version = hashlib.sha256(body).hexdigest()
            records = lambda: linz_coastal(db, json.loads(body)) if source == "linz-coastal" else infomar(json.loads(body))
        elif source == "eba":
            release = json.loads(request(EBA_LATEST)[0])
            archives = [f for f in release["files"] if "transect" in f["key"].lower()
                        and f["key"].lower().endswith(".zip")]
            if len(archives) != 1:
                raise ValueError("Expected one published transect footprint archive")
            version = str(release["id"]) + ":" + archives[0]["checksum"]
            records = lambda: eba_records(release, archives[0])
        elif source in stac:
            version = None  # A paginated collection can change behind an unchanged first page.
            records = lambda: stac_records(db, source, stac[source])
        else:
            collection = collections[source]
            version = cmr_version(collection)
            records = lambda: cmr_records(collection, int(version.split(":")[0]))
        changed = not row or version is None or version != row[1]
        if changed:
            db.execute("DELETE FROM surveys WHERE source=?", (source,))
            db.executemany("INSERT INTO surveys VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?)",
                           ((r[0], source, *r[1:]) for r in records()))
            count = db.execute("SELECT COUNT(*) FROM surveys WHERE source=?", (source,)).fetchone()[0]
            generation = db.execute("PRAGMA user_version").fetchone()[0] + 1
            db.execute(f"PRAGMA user_version={generation}")
            if not count:
                raise ValueError("No geolocated lidar records returned")
        db.execute("INSERT OR REPLACE INTO refreshes VALUES(?,?,?)", (source, time.time(), version))
        return f"{count:,} records" if changed else "unchanged"


def refresh(db, args):
    collections = {**COLLECTIONS, **{c: c for c in args.cmr_collection}}
    stac = {"swisssurface3d": SWISS}
    for url in args.stac_items:
        if not url.startswith("https://"):
            raise ValueError("Supply an HTTPS lidar STAC collection items URL")
        stac["stac-" + hashlib.sha256(url.encode()).hexdigest()[:16]] = url
    if args.source:
        sources = args.source
    elif args.stac_items or args.cmr_collection:
        sources = [name for name in stac if name.startswith("stac-")] + args.cmr_collection
    else:
        sources = ["opentopography", "eba", *MARINE, *collections, *stac]
    failed = False
    for source in dict.fromkeys(sources):
        if source not in ("opentopography", "eba", *MARINE) and source not in collections and source not in stac:
            raise ValueError(f"Unknown source {source}")
        try:
            status = refresh_source(db, source, args.force, collections, stac)
            print(f"{source}: {status}", flush=True)
        except (OSError, ValueError, KeyError, sqlite3.Error, subprocess.SubprocessError) as exc:
            print(f"{source}: {exc}", file=sys.stderr, flush=True)
            failed = True
    return int(failed)


def selected_rows(db, args):
    where, params = [], []
    if getattr(args, "id", None):
        where.append("id=?")
        params.append(args.id)
    if args.source:
        where.append("source=?")
        params.append(args.source)
    if args.bbox:
        w, s, e, n = args.bbox
        if not (-180 <= w <= 180 and -180 <= e <= 180 and -90 <= s <= n <= 90):
            raise ValueError("Use WGS84 west south east north")
        where.append("north>=? AND south<=? AND " +
                     ("(east>=? AND west<=?)" if w <= e else "(east>=? OR west<=?)"))
        params.extend((s, n, w, e))
    query = "SELECT * FROM surveys" + (" WHERE " + " AND ".join(where) if where else "") + " ORDER BY id"
    return db.execute(query, params)


def export(db, args):
    with tempfile.NamedTemporaryFile(dir=args.output.parent, delete=False, mode="w", encoding="utf8") as f:
        try:
            f.write('{"type":"FeatureCollection","features":[')
            count = 0
            for row in selected_rows(db, args):
                props = {k: row[k] for k in ("id", "name", "product", "source", "page", "asset", "date", "stac")}
                if count:
                    f.write(",")
                json.dump({"type": "Feature", "geometry": json.loads(row["geometry"]), "properties": props}, f)
                count += 1
            f.write("]}\n")
            f.close()
            os.replace(f.name, args.output)
        finally:
            Path(f.name).unlink(missing_ok=True)
    print(f"{count:,} footprints in {args.output}")


def eba_archives():
    phrase = '"Discrete airborne LiDAR transects collected by EBA"'
    url = "https://zenodo.org/api/records?" + urlencode({"q": phrase, "size": 25})
    while url:
        body, _ = request(url)
        response = json.loads(body)
        for hit in response["hits"]["hits"]:
            if not hit["metadata"]["title"].startswith("L1A - Discrete airborne LiDAR transects collected by EBA"):
                continue
            for f in hit["files"]:
                if f["key"].lower().endswith(".zip"):
                    yield f["links"]["self"], f["key"], f["size"], f["checksum"]
        url = response.get("links", {}).get("next")


def scan_files(db, args):
    if args.eba_archives:
        yield from eba_archives()
        return
    pages = 0
    for r in selected_rows(db, args):
        if r["asset"] and r["source"] != "eba":
            yield r["asset"], unquote(urlsplit(r["asset"]).path.rsplit("/", 1)[-1]), None, None
        else:
            pages += 1
    if pages:
        print(f"{pages:,} records have publisher or regional archive links; see GeoJSON export.", file=sys.stderr)


def signature_ok(path, name):
    with path.open("rb") as f:
        sig = f.read(4)
    name = name.lower()
    if name.endswith(".zip"):
        return sig.startswith(b"PK")
    if name.endswith(".gz"):
        return sig.startswith(b"\x1f\x8b")
    if name.endswith((".las", ".laz")):
        return sig == b"LASF"
    return sig in (b"II*\0", b"MM\0*", b"II+\0", b"MM\0+")


def download(db, args):
    if args.eba_archives and (args.bbox or args.source or args.id or args.all):
        raise ValueError("Regional EBA archives are a separate download selection")
    if not any((args.bbox, args.source, args.id, args.all, args.eba_archives)):
        raise ValueError("Choose --id, --bbox, --source, --eba-archives, or --all")
    if args.max_files is not None and args.max_files < 1:
        raise ValueError("--max-files must be positive")
    if not args.list and not args.directory:
        raise ValueError("Choose --directory for downloaded files")
    count = 0
    found = False
    for url, name, size, checksum in scan_files(db, args):
        found = True
        if args.list:
            print(f"{name}\t{size if size is not None else ''}\t{url}")
        else:
            # Distinct assets sometimes have the same filename.
            folder = args.directory / hashlib.sha256(url.encode()).hexdigest()[:20]
            folder.mkdir(parents=True, exist_ok=True)
            dest = folder / Path(name).name
            if dest.is_file() and signature_ok(dest, name) and (size is None or dest.stat().st_size == size):
                continue
            with tempfile.TemporaryDirectory(dir=folder) as tmp:
                part = Path(tmp, "download")
                cmd = ["curl", "--fail", "--location", "--silent", "--show-error", "--retry", "2",
                       "--connect-timeout", "20", "--speed-limit", "1", "--speed-time", "120"]
                if args.netrc:
                    cookies = str(Path(tmp, "cookies"))
                    cmd.extend(["--netrc", "--cookie", "", "--cookie-jar", cookies])
                subprocess.run([*cmd, "--output", str(part), "--", url], check=True)
                if not signature_ok(part, name):
                    raise ValueError(f"{name}: expected scan data; check publisher access or Earthdata login")
                if size is not None and part.stat().st_size != size:
                    raise ValueError(f"Incomplete download of {name}")
                if checksum:
                    algorithm, digest = checksum.split(":", 1)
                    with part.open("rb") as f:
                        if hashlib.file_digest(f, algorithm).hexdigest() != digest:
                            raise ValueError(f"Checksum mismatch for {name}")
                os.replace(part, dest)
                print(dest, flush=True)
        count += 1
        if count == args.max_files:
            break
    if not found:
        raise ValueError("No direct scan files in this selection")


def ground_rasters(collection):
    """Iterate georeferenced bare-earth COGs from a publisher collection."""
    body, _ = request(collection)
    for link in json.loads(body)["links"]:
        if link["rel"] != "item" or not urlsplit(link["href"]).path.endswith("_be.json"):
            continue
        item, _ = request(urljoin(collection, link["href"]))
        for a in json.loads(item)["assets"].values():
            if ("cloud-optimized" in a.get("type", "") and "data" in a.get("roles", [])
                    and a["href"].startswith("https://")):
                yield a


def ground_assets(collection, lon, lat, km):
    """Select published bare-earth COGs that intersect this small display window."""
    dy = km / 2 / 110
    dx = min(180, dy / max(.001, math.cos(math.radians(min(89.99, abs(lat) + dy)))))
    assets = []
    for a in ground_rasters(collection):
        w, s, e, n = a["bbox"]
        intersects = any(w <= lon + dx + offset and e >= lon - dx + offset for offset in (0, 360, -360))
        if intersects and n >= lat - dy and s <= lat + dy:
            assets.append("/vsicurl/" + a["href"])
    return assets


def terrain(db, args):
    if not (-180 <= args.lon <= 180 and -90 <= args.lat <= 90 and 0.1 <= args.km <= 50):
        raise ValueError("Use valid coordinates and a window 0.1–50 km wide")
    row = db.execute("SELECT stac FROM surveys WHERE id=?", (args.id,)).fetchone()
    if not row or not row[0]:
        raise ValueError("This survey has no associated public OpenTopography raster collection")
    assets = ground_assets(row[0], args.lon, args.lat, args.km)
    if not assets:
        raise ValueError("No published bare-earth raster covers this window")
    half = str(args.km * 500)
    crs = f"+proj=aeqd +lat_0={args.lat} +lon_0={args.lon} +datum=WGS84 +units=m"
    env = dict(os.environ, GDAL_DISABLE_READDIR_ON_OPEN="EMPTY_DIR", GDAL_HTTP_TIMEOUT="45")
    with tempfile.TemporaryDirectory(dir=args.output.parent) as tmp:
        dem, shade = Path(tmp, "window.tif"), Path(tmp, "relief.tif")
        subprocess.run(["gdalwarp", "-q", "-t_srs", crs, "-te", "-"+half, "-"+half, half, half,
                        "-ts", "1024", "1024", "-r", "bilinear", "-ot", "Float32",
                        "-dstnodata", "-9999", *assets, str(dem)], check=True, env=env)
        info = json.loads(subprocess.check_output(["gdalinfo", "-json", "-mm", str(dem)]))
        if "computedMin" not in info["bands"][0]:
            raise ValueError("Window contains no surveyed terrain")
        subprocess.run(["gdaldem", "hillshade", str(dem), str(shade), "-q", "-multidirectional",
                        "-compute_edges", "-co", "COMPRESS=DEFLATE"], check=True)
        os.replace(shade, args.output)
    print(args.output)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--db", type=Path, default=Path("Data/Lidar/catalog.sqlite"))
    sub = parser.add_subparsers(dest="action", required=True)
    sync = sub.add_parser("refresh", help="Refresh public scan metadata")
    sync.add_argument("--source", action="append", help="Refresh only this source (repeatable)")
    sync.add_argument("--stac-items", action="append", default=[], help="Additional lidar STAC collection items URL (GET pagination)")
    sync.add_argument("--cmr-collection", action="append", default=[], help="Additional NASA airborne lidar collection concept ID")
    sync.add_argument("--force", action="store_true", help="Check upstream even if checked today")
    out = sub.add_parser("export", help="Write coverage and access links as GeoJSON")
    out.add_argument("--source")
    out.add_argument("--bbox", nargs=4, type=float, metavar=("W", "S", "E", "N"))
    out.add_argument("--output", required=True, type=Path)
    bulk = sub.add_parser("download", help="List or fetch published scan files")
    bulk.add_argument("--id")
    bulk.add_argument("--source")
    bulk.add_argument("--bbox", nargs=4, type=float, metavar=("W", "S", "E", "N"))
    bulk.add_argument("--all", action="store_true", help="Select all directly linked granules")
    bulk.add_argument("--eba-archives", action="store_true", help="Discover the Brazilian EBA regional raw ZIP files")
    bulk.add_argument("--list", action="store_true", help="Print filenames, known sizes and URLs without downloading")
    bulk.add_argument("--directory", type=Path)
    bulk.add_argument("--max-files", type=int, help="Stop after this many new files")
    bulk.add_argument("--netrc", action="store_true", help="Use curl netrc credentials for Earthdata downloads")
    render = sub.add_parser("terrain", help="Stream a window of a hosted bare-earth raster")
    render.add_argument("--id", required=True)
    render.add_argument("--lon", required=True, type=float)
    render.add_argument("--lat", required=True, type=float)
    render.add_argument("--km", type=float, default=2)
    render.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    if args.action == "download" and args.eba_archives:
        download(None, args)
        return 0
    if args.action == "refresh":
        args.db.parent.mkdir(parents=True, exist_ok=True)
    elif not args.db.is_file():
        parser.error("Refresh the catalog first")
    with sqlite3.connect(args.db) as db:
        db.row_factory = sqlite3.Row
        if args.action == "refresh":
            db.execute("PRAGMA journal_mode=WAL")
            db.executescript(SCHEMA)
            return refresh(db, args)
        if args.action == "export":
            export(db, args)
        elif args.action == "download":
            download(db, args)
        else:
            terrain(db, args)
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, URLError, ValueError, KeyError, sqlite3.Error, subprocess.SubprocessError) as error:
        sys.exit(str(error))
