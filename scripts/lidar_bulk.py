"""Fetch geographically distributed lidar assets in parallel beside the map catalog."""
import argparse
from contextlib import closing, contextmanager
from collections import defaultdict, deque
from concurrent.futures import FIRST_COMPLETED, ThreadPoolExecutor, wait
from dataclasses import dataclass
import fcntl
import hashlib
import json
import math
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import sqlite3
import sys
import threading
import time
from urllib.parse import unquote, urlsplit
import zipfile

import requests

import lidar_catalog as catalog
from lidar_regions import region


@dataclass(frozen=True)
class Asset:
    survey: str
    url: str
    kind: str
    bounds: tuple
    name: str = ""
    size: int = 0
    checksum: str = ""


SOURCES = ("linz", "infomar", "noaa", "opentopography", "eba", "swiss")
NOAA_INDEX = "https://noaa-nos-coastal-lidar-pds.s3.amazonaws.com/laz/index.html"
SCHEMA = """
CREATE TABLE IF NOT EXISTS countries (folder TEXT PRIMARY KEY);
CREATE TABLE IF NOT EXISTS restores (country TEXT PRIMARY KEY);
CREATE TABLE IF NOT EXISTS digests (url TEXT PRIMARY KEY, checksum TEXT);
CREATE TABLE IF NOT EXISTS files (
 url TEXT PRIMARY KEY, survey TEXT NOT NULL, kind TEXT NOT NULL, path TEXT NOT NULL,
 bytes INTEGER NOT NULL, west REAL, south REAL, east REAL, north REAL
);
CREATE INDEX IF NOT EXISTS by_survey ON files(survey);
CREATE INDEX IF NOT EXISTS by_location ON files(west, east);
CREATE TABLE IF NOT EXISTS transects (
 survey TEXT, path TEXT, member TEXT, west REAL, south REAL, east REAL, north REAL,
 PRIMARY KEY(path,member)
);
CREATE INDEX IF NOT EXISTS by_transect_location ON transects(west,east);
"""
THREAD = threading.local()


def http():
    if not hasattr(THREAD, "session"):
        THREAD.session = requests.Session()
        THREAD.session.headers.update({"User-Agent": "1kEE-lidar-bulk/1", "Accept-Encoding": "identity"})
    return THREAD.session


def spread(rows):
    """Round-robin geographic cells so a large catalog starts in many different places."""
    cells = defaultdict(deque)
    for row in rows:
        cells[math.floor((row["west"] + row["east"])/10),
              math.floor((row["north"] + row["south"])/10)].append(row)
    cells = dict(sorted(cells.items(), key=lambda item: len(item[1])))
    while cells:
        for cell in list(cells):
            yield cells[cell].popleft()
            if not cells[cell]:
                del cells[cell]


def interleave(groups, width=6):
    """Make progress in different surveys even if one has thousands of assets."""
    active = deque()
    for group in groups:
        active.append(iter(group))
        while len(active) >= width:
            current = active.popleft()
            asset = next(current, None)
            if asset is not None:
                yield asset
                active.append(current)
    while active:
        current = active.popleft()
        asset = next(current, None)
        if asset is not None:
            yield asset
            active.append(current)


def noaa_assets(key, index_url):
    # NOAA tile indexes contain the same file URLs and footprints in compact form.
    try:
        body, _ = catalog.request(index_url)
        with tempfile.TemporaryDirectory() as folder:
            index = Path(folder) / "index.zip"
            index.write_bytes(body)
            cmd = ["ogr2ogr", "-f", "GeoJSONSeq", "-t_srs", "EPSG:4326", "-select", "url",
                   "/vsistdout/", "/vsizip/" + str(index)]
            with subprocess.Popen(cmd, stdout=subprocess.PIPE) as proc:
                for line in proc.stdout:
                    f = json.loads(line.lstrip(b"\x1e"))
                    url = f["properties"].get("url") or ""
                    if url.startswith("https://") and url.lower().endswith(".copc.laz"):
                        yield Asset(key, url, "copc", catalog.bounds(f["geometry"]))
                if proc.wait():
                    raise ValueError("Unreadable NOAA tile index")
    except (OSError, ValueError, KeyError) as exc:
        print(f"Inventory {key}: {exc}", file=sys.stderr, flush=True)


def ground_files(row):
    try:
        for asset in catalog.ground_rasters(row["stac"]):
            yield Asset(row["id"], asset["href"], "raster", tuple(asset["bbox"]))
    except (OSError, ValueError, KeyError) as exc:
        print(f"Inventory {row['id']}: {exc}", file=sys.stderr, flush=True)


def inventory(source, db):
    if source == "eba":
        for url, name, size, checksum in sorted(catalog.eba_archives(), key=lambda a: a[2]):
            yield Asset("eba-archives", url, "archive", (None,)*4, name, size, checksum)
        return
    query = {
        "linz": "source='linz-coastal'",
        "infomar": "source='infomar-lidar' AND id LIKE '%:SR_URL'",
        "swiss": "source='swisssurface3d' AND asset LIKE '%.copc.laz'",
        "noaa": "id LIKE 'NOAAID:%'",
        "opentopography": "stac IS NOT NULL",
    }[source]
    rows = spread(db.execute("SELECT * FROM surveys WHERE " + query + " ORDER BY date DESC,id"))
    if source == "noaa":
        index, _ = catalog.request(NOAA_INDEX)
        links = defaultdict(list)
        for url, ident in re.findall(r'(https://noaa-nos-coastal-lidar-pds\.s3\.amazonaws\.com/laz/[^/"\s]+/(\d+)/tileindex_[^"\s<>]+\.zip)', index.decode()):
            links[ident].append(url)
        keys = [row["id"] for row in rows]
        keys.sort(key=lambda key: key != "NOAAID:38")
        def collections():
            for key in keys:
                choices = links.get(key.split(":")[1], [])
                if choices:
                    url = next((v for v in choices if "/geoid18/" in v), choices[0])
                    yield noaa_assets(key, url)
        yield from interleave(collections())
        return
    if source == "opentopography":
        yield from interleave(ground_files(row) for row in rows)
        return
    for row in rows:
        box = tuple(row[k] for k in ("west", "south", "east", "north"))
        checksum = ""
        if source == "linz":
            item = db.execute("SELECT body FROM resources WHERE url=?", (row["id"][5:],)).fetchone()
            if item:
                digest = json.loads(item[0])["assets"]["visual"].get("file:checksum", "")
                if digest.startswith("1220"):
                    checksum = "sha256:" + digest[4:]
        yield Asset(row["id"], row["asset"], {"linz":"raster", "infomar":"relief", "swiss":"copc"}[source],
                    box, checksum=checksum)


class NoSpace(Exception):
    pass


class Space:
    def __init__(self, root, reserve):
        self.root = root
        self.reserve = reserve
        self.remaining = 0
        self.lock = threading.Lock()

    @contextmanager
    def claim(self, size):
        # Account for other writers on the volume and outstanding parallel transfers.
        with self.lock:
            if size > shutil.disk_usage(self.root).free - self.reserve - self.remaining:
                raise NoSpace
            self.remaining += size
        try:
            yield
        finally:
            with self.lock:
                self.remaining -= size


def valid(path, asset, size):
    name = asset.name or urlsplit(asset.url).path
    if not path.is_file() or path.stat().st_size != size or not catalog.signature_ok(path, name):
        return False
    if asset.checksum:
        algorithm, expected = asset.checksum.split(":", 1)
        with path.open("rb") as f:
            return hashlib.file_digest(f, algorithm).hexdigest() == expected
    return True


def transfer(asset, dest, space):
    part = dest.with_suffix(dest.suffix + ".part")
    validator = dest.with_suffix(dest.suffix + ".validator")
    # Only append bytes when protected by a publisher digest or a previous HTTP validator.
    tag = validator.read_text() if validator.is_file() else ""
    offset = part.stat().st_size if part.is_file() and (tag or asset.checksum) else 0
    headers = {"Range": f"bytes={offset}-"} if offset else {}
    if tag and offset:
        headers["If-Range"] = tag
    with http().get(asset.url, headers=headers, stream=True, timeout=(15, 90)) as response:
        if response.status_code == 416 and offset:
            size = asset.size or int(response.headers.get("Content-Range", "*/0").split("/")[-1])
            if valid(part, asset, size):
                os.replace(part, dest)
                validator.unlink(missing_ok=True)
                return size
            part.unlink(missing_ok=True)
        response.raise_for_status()
        size = int(response.headers.get("Content-Length", 0))
        if response.status_code == 206:
            interval = re.fullmatch(r"bytes (\d+)-(\d+)/(\d+)", response.headers.get("Content-Range", ""))
            if not interval or int(interval[1]) != offset or int(interval[2])+1 != int(interval[3]):
                raise ValueError("Incomplete byte range")
            size = int(interval[3])
        else:
            offset = 0
        if (asset.size and size and asset.size != size) or not (asset.size or size):
            raise ValueError("Publisher did not supply a consistent file size")
        size = asset.size or size
        tag = response.headers.get("ETag") or response.headers.get("Last-Modified") or ""
        with space.claim(size - offset):
            if not offset:
                with part.open("wb"):
                    pass
            validator.write_text(tag)
            with part.open("ab") as out:
                for chunk in response.iter_content(1024*1024):
                    if out.tell()+len(chunk) > size:
                        raise ValueError("Response exceeds announced size")
                    if shutil.disk_usage(space.root).free < space.reserve + len(chunk):
                        raise NoSpace
                    out.write(chunk)
            if not valid(part, asset, size):
                part.unlink(missing_ok=True)
                raise ValueError("Incomplete scan, unexpected file type, or checksum mismatch")
            os.replace(part, dest)
            validator.unlink(missing_ok=True)
            return size


def index_archive(db, root, dest):
    # Regional EBA ZIPs contain individual LAS/LAZ transects with publisher IDs.
    rel = str(dest.relative_to(root))
    with sqlite3.connect((root / "catalog.sqlite").resolve().as_uri() + "?mode=ro", uri=True) as surveys:
        with zipfile.ZipFile(dest) as archive:
            for member in archive.namelist():
                match = re.match(r"NP_T-(\d+(?:_\d+)?)(?:[._]|$)", Path(member).name, re.I)
                if not match or not member.lower().endswith((".las", ".laz")):
                    continue
                row = surveys.execute("SELECT id,west,south,east,north FROM surveys WHERE id=?",
                                      ("EBA:" + match[1],)).fetchone()
                if row:
                    db.execute("INSERT OR REPLACE INTO transects VALUES(?,?,?,?,?,?,?)",
                               (row[0], rel, member, *row[1:]))


def destination(root, asset):
    continent, country = region(asset.survey, asset.bounds)
    key = hashlib.sha256(asset.url.encode()).hexdigest()[:20]
    name = Path(asset.name or unquote(urlsplit(asset.url).path)).name
    return root / "scans" / continent / country / (key + "-" + name)


def adopt(old, new):
    if old.is_file() and not new.exists():
        new.parent.mkdir(parents=True, exist_ok=True)
        old.rename(new)


def migrate(db, root):
    for url, survey, kind, path, w, s, e, n in db.execute(
            "SELECT url,survey,kind,path,west,south,east,north FROM files").fetchall():
        old = root / path
        if len(Path(path).parts) != 3:
            continue
        asset = Asset(survey, url, kind, (w,s,e,n), old.name)
        new = destination(root, asset)
        adopt(old, new)
        rel = str(new.relative_to(root))
        db.execute("UPDATE files SET path=? WHERE url=?", (rel, url))
        db.execute("UPDATE transects SET path=? WHERE path=?", (rel, path))
        db.execute("INSERT OR IGNORE INTO countries VALUES(?)", (str(new.parent.relative_to(root)),))
    db.commit()
    # Older workers kept partial requests beside completed files. Retain them for range resume.
    scans = root / "scans"
    for directory in scans.iterdir() if scans.is_dir() else ():
        if not re.fullmatch("[a-f0-9]{20}", directory.name) or not directory.is_dir():
            continue
        for partial in directory.iterdir():
            if partial.suffix in (".part", ".validator"):
                adopt(partial, root / "bulk-incoming" / directory.name / partial.name)
        if not any(directory.iterdir()):
            directory.rmdir()


FILE_LOCKS = [threading.Lock() for _ in range(64)]
REGIONS_LOCK = threading.Lock()


def fetch(asset, root, space, restore=False):
    key = hashlib.sha256(asset.url.encode()).hexdigest()[:20]
    with FILE_LOCKS[int(key, 16) % len(FILE_LOCKS)]:
        try:
            return fetch_locked(asset, root, space, key, restore)
        except requests.HTTPError as exc:
            if asset.kind != "relief" or exc.response.status_code not in (404, 410):
                raise
            with closing(sqlite3.connect((root / "catalog.sqlite").resolve().as_uri() + "?mode=ro", uri=True)) as db:
                alternate = db.execute("SELECT asset FROM surveys WHERE id=?",
                                       (asset.survey.removesuffix(":SR_URL") + ":XYZ_URL",)).fetchone()
            if not alternate or "_WGS84_LAT_XYZ_" not in alternate[0]:
                raise
    # The same acquisition may still have its measured depth grid after a relief link disappears.
    return fetch(Asset(asset.survey, alternate[0], "soundings", asset.bounds), root, space, restore)


def fetch_locked(asset, root, space, key, restore):
    with closing(sqlite3.connect(root / "bulk.sqlite", timeout=60)) as db, db:
        saved = db.execute("SELECT bytes,path FROM files WHERE url=?", (asset.url,)).fetchone()
        dest = root / saved[1] if saved else destination(root, asset)
        name = asset.name or unquote(urlsplit(asset.url).path.split("/")[-1])
        with REGIONS_LOCK:
            relative = str(dest.parent.relative_to(root))
            known = db.execute("SELECT 1 FROM countries WHERE folder=?", (relative,)).fetchone()
            if known and not dest.parent.is_dir() and not restore:
                return
            dest.parent.mkdir(parents=True, exist_ok=True)
            db.execute("INSERT OR IGNORE INTO countries VALUES(?)", (relative,))
            if asset.checksum:
                db.execute("INSERT OR REPLACE INTO digests VALUES(?,?)", (asset.url, asset.checksum))
            db.commit()
        if saved and dest.is_file() and dest.stat().st_size == saved[0]:
            if asset.kind == "archive" and not db.execute("SELECT 1 FROM transects WHERE path=? LIMIT 1", (saved[1],)).fetchone():
                index_archive(db, root, dest)
            return
        for suffix in (".part", ".validator"):
            adopt(root / "bulk-incoming" / key / (name + suffix), Path(str(dest) + suffix))
        for attempt in range(4):
            try:
                size = transfer(asset, dest, space)
                break
            except (requests.RequestException, ValueError) as exc:
                if isinstance(exc, requests.HTTPError) and exc.response.status_code in (401, 403, 404, 410):
                    raise
                if attempt == 3:
                    raise
                print(f"Retry {asset.url}: {exc}", file=sys.stderr, flush=True)
                time.sleep(2**attempt)
        if asset.kind == "archive":
            index_archive(db, root, dest)
        db.execute("INSERT OR REPLACE INTO files VALUES(?,?,?,?,?,?,?,?,?)",
                   (asset.url, asset.survey, asset.kind, str(dest.relative_to(root)), size, *asset.bounds))
    print(f"Saved {size:,} bytes  {asset.survey}  {name}", flush=True)


def run_source(source, args, space):
    failed = 0
    with sqlite3.connect(args.db.resolve().as_uri() + "?mode=ro", uri=True) as db, ThreadPoolExecutor(args.per_source) as pool:
        db.row_factory = sqlite3.Row
        pending = {}
        seen = set()
        assets = iter(inventory(source, db))
        exhausted = False
        while pending or not exhausted:
            while not exhausted and len(pending) < args.per_source:
                asset = next(assets, None)
                if asset is None:
                    exhausted = True
                elif asset.url not in seen:
                    seen.add(asset.url)
                    pending[pool.submit(fetch, asset, args.db.parent, space)] = asset
            if not pending:
                continue
            for task in wait(pending, return_when=FIRST_COMPLETED).done:
                asset = pending.pop(task)
                try:
                    task.result()
                except NoSpace:
                    print(f"{source}: reached disk budget", flush=True)
                    exhausted = True
                except (OSError, ValueError, requests.RequestException) as exc:
                    print(f"Failed {asset.url}: {exc}", file=sys.stderr, flush=True)
                    failed += 1
    return failed


def restore_country(root, country, space):
    with sqlite3.connect(root / "bulk.sqlite", timeout=60) as db:
        rows = db.execute("SELECT f.survey,f.url,f.kind,f.west,f.south,f.east,f.north,f.path,f.bytes,d.checksum "
                          "FROM files f LEFT JOIN digests d USING(url) WHERE f.path LIKE ? ESCAPE '\\'",
                          ("scans/%/" + country.replace("\\", "\\\\").replace("%", "\\%").replace("_", "\\_") + "/%",)).fetchall()
    if not rows:
        raise ValueError("No saved source records for country " + country)
    failed = 0
    for survey, url, kind, w, s, e, n, path, size, digest in rows:
        asset = Asset(survey, url, kind, (w,s,e,n), Path(path).name[21:], size, digest or "")
        try:
            fetch(asset, root, space, restore=True)
        except NoSpace:
            print(f"Restore of {country} paused at disk reserve", flush=True)
            return 1
        except (OSError, requests.RequestException, ValueError) as exc:
            print(f"Failed {url}: {exc}", file=sys.stderr, flush=True)
            failed += 1
    return failed


def requested_restores(root, space, finished):
    failed = 0
    while True:
        with sqlite3.connect(root / "bulk.sqlite", timeout=60) as db:
            row = db.execute("SELECT country FROM restores LIMIT 1").fetchone()
        if row:
            try:
                failed += restore_country(root, row[0], space)
            except (OSError, ValueError, sqlite3.Error) as exc:
                print(f"Could not restore {row[0]}: {exc}", file=sys.stderr, flush=True)
                failed += 1
            with sqlite3.connect(root / "bulk.sqlite", timeout=60) as db:
                db.execute("DELETE FROM restores WHERE country=?", row)
        elif finished.wait(1):
            return failed


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--db", type=Path, required=True, help="Existing Lidar/catalog.sqlite on the target drive")
    p.add_argument("--source", choices=SOURCES, action="append", help="Repeat to limit providers")
    p.add_argument("--per-source", type=int, default=2, help="Simultaneous transfers per provider")
    p.add_argument("--reserve-gb", type=float, default=100, help="Space to leave free for maps and other disk users")
    p.add_argument("--restore-country", metavar="COUNTRY", help="Restore previously saved files for one country")
    args = p.parse_args()
    if not args.db.is_file() or not 1 <= args.per_source <= 8 or not math.isfinite(args.reserve_gb) or args.reserve_gb < 0:
        p.error("Supply an existing catalog, 1–8 workers per source, and a nonnegative reserve")
    sources = sorted(set(args.source or SOURCES))
    with (args.db.parent / "bulk.lock").open("a") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            if not args.restore_country:
                p.error("A bulk download is already running")
            with sqlite3.connect(args.db.parent / "bulk.sqlite", timeout=60) as db:
                db.execute("INSERT OR IGNORE INTO restores VALUES(?)", (args.restore_country,))
            print("Queued country restore: " + args.restore_country)
            return 0
        with sqlite3.connect(args.db.parent / "bulk.sqlite") as db:
            db.executescript(SCHEMA)
            migrate(db, args.db.parent)
        space = Space(args.db.parent, int(args.reserve_gb*1e9))
        if args.restore_country:
            return int(bool(restore_country(args.db.parent, args.restore_country, space)))
        print(f"{len(sources)*args.per_source} parallel slots; {shutil.disk_usage(args.db.parent).free:,} bytes free; "
              f"reserve {space.reserve:,} bytes", flush=True)
        with ThreadPoolExecutor(len(sources)+1) as pool:
            finished = threading.Event()
            restores = pool.submit(requested_restores, args.db.parent, space, finished)
            tasks = {pool.submit(run_source, src, args, space): src for src in sources}
            failed = 0
            for task in tasks:
                try:
                    failed += task.result()
                except Exception as exc:
                    print(f"Inventory {tasks[task]} failed: {exc}", file=sys.stderr, flush=True)
                    failed += 1
            finished.set()
            failed += restores.result()
        return int(bool(failed))


if __name__ == "__main__":
    sys.exit(main())
