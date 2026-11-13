"""Render a georeferenced map tile from the local survey index or published measurements."""
import argparse
from collections import defaultdict, deque
from contextlib import ExitStack
import gzip
import hashlib
import io
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
import zipfile
from urllib.error import HTTPError
from urllib.parse import quote, unquote, urlencode, urlsplit

import numpy as np
from PIL import Image, ImageChops, ImageDraw, PngImagePlugin
import lidar_catalog as catalog

SIZE = 512
ACTIVE = None


def check_view():
    if ACTIVE is not None and not ACTIVE.is_file():
        raise SystemExit("Camera moved")

NOAA_INDEX = "https://noaa-nos-coastal-lidar-pds.s3.amazonaws.com/laz/index.html"


def save(image, path, credits=()):
    path.parent.mkdir(parents=True, exist_ok=True)
    info = PngImagePlugin.PngInfo()
    info.add_text("Surveys", json.dumps(list(credits)))
    with tempfile.NamedTemporaryFile(dir=path.parent, delete=False) as tmp:
        try:
            image.save(tmp, format="PNG", pnginfo=info)
            tmp.close()
            os.replace(tmp.name, path)
        finally:
            Path(tmp.name).unlink(missing_ok=True)


def tile_bounds(z, x, y):
    span = 180 / (2 ** z)
    w, n = -180 + x * span, 90 - y * span
    return w, n - span, w + span, n


PUBLISHED = """(id LIKE 'NOAAID:%' OR id LIKE 'USGS_3DEP_ID:%' OR stac IS NOT NULL
               OR source IN ('linz-coastal','swisssurface3d')
               OR (source='infomar-lidar' AND id LIKE '%:SR_URL'))"""


def survey_filter(column, preferred):
    if not preferred:
        return "", ()
    # A selected LINZ collection includes all of its individual COG tiles.
    if preferred.startswith("LINZ:") and preferred.endswith("/"):
        return f" AND instr({column}, ?) = 1", (preferred,)
    return f" AND {column} = ?", (preferred,)


def matching(db, box, measurements=False, preferred="", local=False):
    w, s, e, n = box
    nasa = "(product IN ('NASA lidar raster','NASA lidar point cloud') AND asset IS NOT NULL)"
    fields = ("id,source,name,product,geometry,asset,stac,page,west,south,east,north" if measurements
              else f"geometry, ({PUBLISHED} OR {nasa}) AS available")
    sql = f"SELECT {fields} FROM surveys WHERE west<? AND east>=? AND south<? AND north>=?"
    clause, args = survey_filter("id", preferred)
    sql += clause
    if measurements:
        selected_nasa = f" OR {nasa}" if preferred else ""
        sql += f""" AND ({PUBLISHED} OR product='NASA lidar raster' OR product='Lidar raster'
                    OR (source='eba' AND {int(local)}){selected_nasa})
                    ORDER BY source='linz-coastal' DESC, source='infomar-lidar' DESC,
                    stac IS NOT NULL DESC, date DESC, id"""
    return db.execute(sql, (e, w, n, s, *args))



def draw_geometry(draw, geometry, box, color, filled=False):
    w, s, e, n = box
    def point(coord):
        return ((coord[0] - w) * SIZE / (e - w), (n - coord[1]) * SIZE / (n - s))
    if geometry["type"] == "Point":
        x, y = point(geometry["coordinates"])
        draw.ellipse((x-1, y-1, x+1, y+1), fill=color)
        return
    for polygon in catalog.polygons(geometry):
        for i, ring in enumerate(polygon):
            if len(ring) < 3:
                continue
            coords = [point(p) for p in ring]
            # Unwrap only rings crossing the date line; ordinary survey tiles stay cheap.
            if any(abs(a[0]-b[0]) > 180 for a, b in zip(ring, ring[1:])):
                lon = ring[0][0]
                coords = []
                for coord in ring:
                    lon += (coord[0] - lon + 180) % 360 - 180
                    coords.append(point((lon, coord[1])))
                shift = round((SIZE/2 - coords[0][0]) / (360 * SIZE / (e-w))) * (360 * SIZE / (e-w))
                coords = [(x+shift, y) for x, y in coords]
            if filled:
                draw.polygon(coords, fill=color if i == 0 else 0)
            else:
                draw.line(coords, fill=color, width=1)


def paths(db, box):
    archives = Image.new("RGBA", (SIZE, SIZE))
    published = Image.new("RGBA", (SIZE, SIZE))
    archive_paths, map_paths = ImageDraw.Draw(archives), ImageDraw.Draw(published)
    for row in matching(db, box):
        check_view()
        if row["available"]:
            draw_geometry(map_paths, json.loads(row["geometry"]), box, (41, 210, 214, 190))
        else:
            draw_geometry(archive_paths, json.loads(row["geometry"]), box, (148, 166, 184, 170))
    return Image.alpha_composite(archives, published)



def cached_bytes(folder, url, max_age):
    check_view()
    key = hashlib.sha256(url.encode()).hexdigest()
    path = folder / key[:2] / key
    if path.is_file() and time.time() - path.stat().st_mtime < max_age:
        return path.read_bytes()
    try:
        body, _ = catalog.request(url)
    except OSError:
        if path.is_file():
            return path.read_bytes()
        raise
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(dir=path.parent, delete=False) as tmp:
        try:
            tmp.write(body)
            tmp.close()
            os.replace(tmp.name, path)
        finally:
            Path(tmp.name).unlink(missing_ok=True)
    return body


def elevation_image(values, box):
    valid = np.isfinite(values)
    data = np.where(valid, values, 0.)
    # Blue water-depth elevations, warm land elevations. Preserve unsampled pixels as transparent.
    levels = [-150, -45, -12, -2, 0, 3, 12, 80, 800, 5000]
    colors = [(36,66,130), (50,114,170), (72,172,190), (126,201,190), (175,211,174),
              (210,197,137), (212,173,110), (180,151,114), (176,166,161), (240,236,228)]
    rgb = np.stack([np.interp(data, levels, band) for band in zip(*colors)], axis=-1)
    w, s, e, n = box
    mx = max(.01, (e-w) * 111320 * math.cos(math.radians((s+n)/2)) / SIZE)
    my = max(.01, (n-s) * 111320 / SIZE)
    gx = np.diff(data, axis=1, prepend=data[:, :1]) / mx
    gy = np.diff(data, axis=0, prepend=data[:1, :]) / my
    gx[:, 1:] *= valid[:, :-1]
    gy[1:, :] *= valid[:-1, :]
    light = np.clip(1 + .65 * (gy-gx) / np.sqrt(1 + gx*gx + gy*gy), .45, 1.3)
    return Image.fromarray(np.dstack((np.clip(rgb * light[..., None], 0, 255), valid*255)).astype("uint8"))


def ept_base(identifier, folder):
    provider, key = identifier.split(":", 1)
    if provider == "USGS_3DEP_ID":
        return "https://s3-us-west-2.amazonaws.com/usgs-lidar-public/" + quote(key, safe="")
    body = cached_bytes(folder, NOAA_INDEX, 7 * catalog.DAY).decode()
    pattern = r'https://noaa-nos-coastal-lidar-pds\.s3\.amazonaws\.com/entwine/[^"\s<>]+/' + re.escape(key) + r'/ept\.json'
    links = re.findall(pattern, body)
    if links:
        return next((v for v in links if "/geoid18/" in v), links[0]).rsplit("/", 1)[0]


def ept_returns(row, box, folder):
    import laspy
    from pyproj import CRS, Transformer
    base = ept_base(row["id"], folder)
    if not base:
        return None

    def get(relative):
        return cached_bytes(folder, base + "/" + relative, 30 * catalog.DAY)

    try:
        meta = json.loads(get("ept.json"))
    except HTTPError as exc:
        if exc.code == 404:
            return None
        raise
    if meta["dataType"] != "laszip":
        raise ValueError("Unsupported EPT point encoding: " + meta["dataType"])
    crs = CRS.from_user_input(meta["srs"]["wkt"]).to_2d()
    to_native = Transformer.from_crs(4326, crs, always_xy=True)
    to_map = Transformer.from_crs(crs, 4326, always_xy=True)
    left, bottom, right, top = to_native.transform_bounds(*box)
    if not all(math.isfinite(v) for v in (left, bottom, right, top)):
        return None
    xmin, ymin, _, xmax, ymax, _ = meta["bounds"]
    target = max(right-left, top-bottom) / SIZE
    max_depth = max(0, math.ceil(math.log2((xmax-xmin) / (meta["span"] * max(target, 1e-9)))))
    hierarchy = json.loads(get("ept-hierarchy/0-0-0-0.json"))
    queue = deque([(0, 0, 0, 0)])
    sums = np.zeros(SIZE*SIZE)
    counts = np.zeros(SIZE*SIZE, dtype=np.uint32)
    w, s, e, n = box
    while queue:
        depth, x, y, z = queue.popleft()
        width, height = (xmax-xmin) / (2**depth), (ymax-ymin) / (2**depth)
        if (xmin+x*width > right or xmin+(x+1)*width < left or
                ymin+y*height > top or ymin+(y+1)*height < bottom):
            continue
        node = f"{depth}-{x}-{y}-{z}"
        count = hierarchy.get(node, 0)
        if count == -1:
            hierarchy.update(json.loads(get("ept-hierarchy/" + node + ".json")))
            count = hierarchy[node]
        if count <= 0:
            continue
        cloud = laspy.read(io.BytesIO(get("ept-data/" + node + ".laz")))
        xs, ys = np.asarray(cloud.x), np.asarray(cloud.y)
        keep = ((xs >= left) & (xs <= right) & (ys >= bottom) & (ys <= top)
                & ~np.isin(cloud.classification, [7, 18]) & (np.asarray(cloud.withheld) == 0))
        lon, lat = to_map.transform(xs[keep], ys[keep])
        col = np.floor((lon-w)*SIZE/(e-w)).astype(int)
        line = np.floor((n-lat)*SIZE/(n-s)).astype(int)
        ok = (col >= 0) & (col < SIZE) & (line >= 0) & (line < SIZE)
        indices = line[ok]*SIZE + col[ok]
        np.add.at(sums, indices, np.asarray(cloud.z)[keep][ok])
        np.add.at(counts, indices, 1)
        if depth < max_depth:
            queue.extend((depth+1, 2*x+a, 2*y+b, 2*z+c) for a in (0,1) for b in (0,1) for c in (0,1))
    values = np.divide(sums, counts, out=np.full_like(sums, np.nan), where=counts>0)
    return elevation_image(values.reshape(SIZE, SIZE), box)


def raster_image(urls, box, src_crs=None, depth=False):
    import rasterio
    from rasterio.transform import from_bounds
    from rasterio.enums import Resampling
    from rasterio.vrt import WarpedVRT

    values = np.full((SIZE, SIZE), np.nan, dtype="float32")
    transform = from_bounds(*box, SIZE, SIZE)
    with rasterio.Env(GDAL_DISABLE_READDIR_ON_OPEN="EMPTY_DIR", GDAL_HTTP_TIMEOUT="45"):
        for url in urls:
            check_view()
            with rasterio.open(url) as ds, WarpedVRT(ds, src_crs=src_crs or ds.crs, crs="EPSG:4326", transform=transform,
                    width=SIZE, height=SIZE, resampling=Resampling.bilinear, dtype="float32",
                    nodata=float("nan")) as vrt:
                data = vrt.read(1, masked=True).filled(np.nan)
                np.copyto(values, data, where=np.isfinite(data))
    return elevation_image(-values if depth else values, box)


def irish_relief(asset, box, soundings=False):
    archive = "/vsizip/{" + ("/vsicurl/" if asset.startswith("https://") else "") + asset + "}"
    env = dict(os.environ, GDAL_DISABLE_READDIR_ON_OPEN="EMPTY_DIR", GDAL_HTTP_TIMEOUT="45")
    listing = subprocess.check_output(["gdal", "vsi", "list", "-f", "text", "--recursive", archive],
                                      text=True, env=env, timeout=60)
    extensions = (".xyz",) if soundings else (".tif", ".tiff")
    files = [archive + "/" + p.strip().lstrip("/") for p in listing.splitlines()
             if p.strip().lower().endswith(extensions)]
    if not files and listing.strip().rstrip("/") == archive:
        # GDAL opens a single-member ZIP directly as a dataset.
        files = [archive]
    if not files:
        raise ValueError("Missing survey data in the INFOMAR archive")
    if soundings:
        return raster_image(files, box, src_crs="EPSG:4326", depth=True)
    with tempfile.TemporaryDirectory() as tmp:
        out, png = Path(tmp, "relief.tif"), Path(tmp, "relief.png")
        subprocess.run(["gdalwarp", "-q", "-t_srs", "EPSG:4326", "-te", *map(str, box),
                        "-ts", str(SIZE), str(SIZE), "-r", "bilinear", "-dstalpha", *files, str(out)],
                       check=True, env=env, timeout=180)
        subprocess.run(["gdal_translate", "-q", "-of", "PNG", str(out), str(png)], check=True, timeout=30)
        with Image.open(png) as img:
            return img.convert("RGBA")


SWISS_LAYER = "ch.swisstopo.swisssurface3d-reliefschattierung-multidirektional"


def swiss_relief(box, folder):
    params = dict(SERVICE="WMS", VERSION="1.1.1", REQUEST="GetMap", LAYERS=SWISS_LAYER, STYLES="",
                  SRS="EPSG:4326", BBOX=",".join(map(str, box)), WIDTH=SIZE, HEIGHT=SIZE,
                  FORMAT="image/png", TRANSPARENT="TRUE")
    data = cached_bytes(folder, "https://wms.geo.admin.ch/?" + urlencode(params), 30 * catalog.DAY)
    with Image.open(io.BytesIO(data)) as img:
        return img.convert("RGBA")


def nasa_image(row, box, folder, download):
    url = row["asset"]
    key = hashlib.sha256(url.encode()).hexdigest()[:20]
    name = Path(unquote(urlsplit(url).path)).name
    root = folder.parent
    scans = root / "scans"
    # Recognize granules already acquired with the catalog CLI.
    path = scans / key / name
    if not path.is_file() or not catalog.signature_ok(path, name):
        if row["source"] == "nasa-brazil":
            place = ("South America", "Brazil")
        else:
            from lidar_regions import region
            place = region(row["id"], [row[k] for k in ("west", "south", "east", "north")])
        path = scans.joinpath(*place, key + "-" + name)
        if download:
            catalog.check_removable_mount(root)
            catalog.fetch_scan(url, path, netrc=True)
        elif not path.is_file():
            return None
    check_view()
    if row["product"] == "NASA lidar raster":
        return raster_image([str(path)], box)
    return local_cloud([("scan", path, None)], box)


def survey_image(row, box, folder):
    if row["id"].startswith(("NOAAID:", "USGS_3DEP_ID:")):
        return ept_returns(row, box, folder)
    if row["source"] == "infomar-lidar":
        return irish_relief(row["asset"], box)
    if row["source"] == "swisssurface3d":
        return swiss_relief(box, folder)
    if row["stac"]:
        w, s, e, n = box
        files = catalog.ground_assets(row["stac"], (w+e)/2, (s+n)/2, (n-s)*114.)
        return raster_image(files, box) if files else None
    if row["asset"]:
        url = row["asset"]
        if ".earthdata" in urlsplit(url).hostname:
            local = folder.parent / "scans" / hashlib.sha256(url.encode()).hexdigest()[:20] / Path(urlsplit(url).path).name
            if not local.is_file():
                return None
            url = str(local)
        return raster_image([url], box)


def offline_assets(root, box, preferred=""):
    result = defaultdict(list)
    index = root / "bulk.sqlite"
    if index.is_file():
        with sqlite3.connect(index.resolve().as_uri() + "?mode=ro", uri=True) as db:
            w, s, e, n = box
            query = "SELECT survey,kind,path,url FROM files WHERE west<? AND east>=? AND south<? AND north>=?"
            clause, args = survey_filter("survey", preferred)
            for survey, kind, path, url in db.execute(query + clause, (e, w, n, s, *args)):
                file = root / path
                result[survey].append((kind, file if file.is_file() else url, None))
            if db.execute("SELECT 1 FROM sqlite_master WHERE name='transects'").fetchone():
                clause, args = survey_filter("t.survey", preferred)
                for survey, path, member, url in db.execute(
                        "SELECT t.survey,t.path,t.member,f.url FROM transects t JOIN files f ON t.path=f.path "
                        "WHERE t.west<? AND t.east>=? AND t.south<? AND t.north>=?" + clause,
                        (e, w, n, s, *args)):
                    file = root / path
                    result[survey].append(("laz", file if file.is_file() else url, member))
    return result


def local_cloud(files, box):
    import laspy
    from laspy.copc import Bounds
    from pyproj import Transformer
    w, s, e, n = box
    sums = np.zeros(SIZE*SIZE)
    counts = np.zeros(SIZE*SIZE, dtype=np.uint32)
    for kind, path, member in files:
        check_view()
        with ExitStack() as stack:
            if kind == "copc":
                reader = stack.enter_context(laspy.CopcReader.open(path, http_num_threads=4))
            elif kind == "scan":
                stream = stack.enter_context(gzip.open(path, "rb")) if path.suffix.lower() == ".gz" else path
                reader = stack.enter_context(laspy.open(stream))
            elif isinstance(path, str):
                tmp = stack.enter_context(tempfile.TemporaryDirectory())
                scan = Path(tmp, "transect.laz")
                subprocess.run(["gdal", "vsi", "copy", "-q", "/vsizip/{/vsicurl/" + path + "}/" + member,
                                str(scan)], check=True, timeout=600)
                reader = stack.enter_context(laspy.open(scan))
            else:
                archive = stack.enter_context(zipfile.ZipFile(path))
                stream = stack.enter_context(archive.open(member))
                reader = stack.enter_context(laspy.open(stream))
            crs = reader.header.parse_crs()
            if crs is None:
                raise ValueError(f"Missing point-cloud coordinate system in {member or path}")
            crs = crs.to_2d()
            native = Transformer.from_crs(4326, crs, always_xy=True).transform_bounds(*box)
            to_map = Transformer.from_crs(crs, 4326, always_xy=True)
            x0, y0, x1, y1 = native
            if kind == "copc":
                bounds = Bounds(np.array([x0, y0]), np.array([x1, y1]))
                spacing = max(x1-x0, y1-y0) / SIZE
                chunks = [reader.query(bounds=bounds, resolution=spacing)]
            else:
                chunks = reader.chunk_iterator(250_000)
            for points in chunks:
                check_view()
                x, y = np.asarray(points.x), np.asarray(points.y)
                keep = ((x >= x0) & (x <= x1) & (y >= y0) & (y <= y1)
                        & ~np.isin(points.classification, [7, 18]) & (np.asarray(points.withheld) == 0))
                lon, lat = to_map.transform(x[keep], y[keep])
                col = np.floor((lon-w)*SIZE/(e-w)).astype(int)
                line = np.floor((n-lat)*SIZE/(n-s)).astype(int)
                ok = (col >= 0) & (col < SIZE) & (line >= 0) & (line < SIZE)
                indices = line[ok]*SIZE + col[ok]
                np.add.at(sums, indices, np.asarray(points.z)[keep][ok])
                np.add.at(counts, indices, 1)
    values = np.divide(sums, counts, out=np.full_like(sums, np.nan), where=counts>0)
    return elevation_image(values.reshape(SIZE, SIZE), box)


def offline_image(files, box):
    kind, path, _ = files[0]
    if kind in ("relief", "soundings"):
        return irish_relief(str(path), box, soundings=kind == "soundings")
    if kind == "raster":
        return raster_image([str(path) for _, path, _ in files], box)
    return local_cloud(files, box)


def measurements(db, box, folder, preferred):
    local = offline_assets(folder.parent, box, preferred)
    rows = matching(db, box, True, preferred, bool(local))
    if local:
        on_disk = {survey for survey, files in local.items() if any(isinstance(path, Path) for _, path, _ in files)}
        rows = sorted(rows, key=lambda r: r["id"] not in on_disk)
    result = Image.new("RGBA", (SIZE, SIZE))
    credits = []
    covered = Image.new("L", result.size)
    errors = []
    tried_swiss = False
    for row in rows:
        check_view()
        files = local.get(row["id"])
        if not files and (row["source"] == "eba" or (row["source"] == "swisssurface3d" and tried_swiss)):
            continue
        mask = Image.new("L", result.size)
        draw_geometry(ImageDraw.Draw(mask), json.loads(row["geometry"]), box, 255, filled=True)
        if not ImageChops.subtract(mask, covered).getbbox():
            continue
        if not files and row["source"] == "swisssurface3d":
            tried_swiss = True
        try:
            if preferred and not files and row["source"] == "swisssurface3d" and row["asset"].endswith(".copc.laz"):
                files = [("copc", row["asset"], None)]
            if files:
                image = offline_image(files, box)
            elif row["product"] in ("NASA lidar raster", "NASA lidar point cloud") and row["asset"]:
                image = nasa_image(row, box, folder, download=row["id"] == preferred)
            else:
                image = survey_image(row, box, folder)
        except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as exc:
            errors.append(f"{row['name']}: {exc}")
            if row["id"] == preferred:
                raise
            continue
        if image is None or not image.getbbox():
            continue
        result = Image.alpha_composite(image, result)
        credit = (row["name"], row["id"], row["page"])
        if not files and row["source"] == "swisssurface3d":
            credit = ("swisstopo · swissSURFACE3D Raster shaded surface", SWISS_LAYER,
                      "https://www.swisstopo.admin.ch/en/height-model-swisssurface3d-raster")
        credits.append(credit)
        covered = ImageChops.lighter(covered, mask)
        if row["id"] == preferred or result.getchannel("A").getextrema()[0] == 255:
            break
        # At country scale show a measured preview. Local tiles fill adjacent survey strips.
        if box[2] - box[0] > 0.36:
            break
    if errors and not credits:
        raise ValueError(errors[0])
    return result, credits


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("db", type=Path)
    p.add_argument("kind", choices=["paths", "detail"])
    p.add_argument("z", type=int)
    p.add_argument("x", type=int)
    p.add_argument("y", type=int)
    p.add_argument("output", type=Path)
    p.add_argument("preferred", nargs="?", default="")
    p.add_argument("--empty", type=Path, help="Cache an empty tile separately from imagery")
    p.add_argument("--active", type=Path, help="Stop when this viewport marker disappears")
    a = p.parse_args()
    global ACTIVE
    ACTIVE = a.active
    check_view()
    catalog.check_removable_mount(a.db)
    with sqlite3.connect(a.db.resolve().as_uri() + "?mode=ro", uri=True) as db:
        db.row_factory = sqlite3.Row
        box = tile_bounds(a.z, a.x, a.y)
        if a.kind == "paths":
            image, credits = paths(db, box), []
        else:
            catalog.set_scratch(a.db.parent)
            image, credits = measurements(db, box, a.db.parent / "map-source-cache", a.preferred)
        if a.empty and not image.getbbox():
            a.empty.parent.mkdir(parents=True, exist_ok=True)
            a.empty.touch()
        else:
            save(image, a.output, credits)


if __name__ == "__main__":
    try:
        main()
    except catalog.EarthdataAccessError as error:
        print(error, file=sys.stderr)
        sys.exit(77)  # EX_NOPERM: the UI can pause this selection until the user retries.
