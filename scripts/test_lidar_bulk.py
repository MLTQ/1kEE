from contextlib import closing
import hashlib
from pathlib import Path
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import lidar_bulk as bulk
import lidar_map as maps
from lidar_regions import region


class Response:
    def __init__(self, body, offset=0):
        self.body = body[offset:]
        self.status_code = 206 if offset else 200
        self.headers = {"Content-Length": str(len(self.body)), "ETag": '"version"'}
        if offset:
            self.headers["Content-Range"] = f"bytes {offset}-{len(body)-1}/{len(body)}"

    def __enter__(self):
        return self

    def __exit__(self, *args):
        pass

    def raise_for_status(self):
        pass

    def iter_content(self, _):
        yield self.body


class BulkTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        with closing(sqlite3.connect(self.root / "bulk.sqlite")) as db:
            db.executescript(bulk.SCHEMA)
        self.space = bulk.Space(self.root, 0)
        self.body = b"II*\0fixture"
        self.asset = bulk.Asset("LINZ:one", "https://example.org/data.tiff", "raster", (174,-36,175,-35),
                                checksum="sha256:" + hashlib.sha256(self.body).hexdigest())

    def test_country_eviction_and_restore_reuses_receipt_and_verifies_bytes(self):
        with patch.object(bulk.http(), "get", return_value=Response(self.body)) as get:
            bulk.fetch(self.asset, self.root, self.space)
            dest = bulk.destination(self.root, self.asset)
            self.assertEqual(dest.parent, self.root / "scans/Oceania/New Zealand")
            bulk.fetch(self.asset, self.root, self.space)
            shutil.rmtree(dest.parent)
            bulk.fetch(self.asset, self.root, self.space)
            self.assertFalse(dest.exists())
            self.assertEqual(get.call_count, 1)
            self.assertEqual(maps.offline_assets(self.root, self.asset.bounds)[self.asset.survey][0][1], self.asset.url)
            self.assertEqual(bulk.restore_country(self.root, "New Zealand", self.space), 0)
            self.assertEqual(dest.read_bytes(), self.body)
            self.assertEqual(get.call_count, 2)

    def test_partial_resume_and_reserve(self):
        dest = self.root / "data.tiff"
        Path(str(dest)+".part").write_bytes(self.body[:4])
        Path(str(dest)+".validator").write_text('"version"')
        with patch.object(bulk.http(), "get", return_value=Response(self.body, 4)) as get:
            with self.assertRaises(bulk.NoSpace):
                bulk.transfer(self.asset, dest, bulk.Space(self.root, shutil.disk_usage(self.root).free))
            self.assertFalse(dest.exists())
            bulk.transfer(self.asset, dest, self.space)
            self.assertEqual(get.call_args.kwargs["headers"], {"Range":"bytes=4-", "If-Range":'"version"'})
            self.assertEqual(dest.read_bytes(), self.body)

    def test_old_completed_file_migrates_without_a_transfer(self):
        key = hashlib.sha256(self.asset.url.encode()).hexdigest()[:20]
        old = self.root / "scans" / key / "data.tiff"
        old.parent.mkdir(parents=True)
        old.write_bytes(self.body)
        with closing(sqlite3.connect(self.root / "bulk.sqlite")) as db, db:
            db.execute("INSERT INTO files VALUES(?,?,?,?,?,?,?,?,?)", (self.asset.url,self.asset.survey,"raster",
                       str(old.relative_to(self.root)),len(self.body),*self.asset.bounds))
            bulk.migrate(db, self.root)
            new = self.root / db.execute("SELECT path FROM files").fetchone()[0]
        self.assertEqual(new.read_bytes(), self.body)
        self.assertFalse(old.exists())

    def test_geographic_assignment(self):
        self.assertEqual(region("opentopoID:test", (-92,17,-91,18)), ("North America", "Mexico"))
        self.assertEqual(region("eba-archives", (None,)*4), ("South America", "Brazil"))

    def test_wrong_volume_rejected_before_any_write(self):
        catalog = self.root / "catalog.sqlite"
        catalog.touch()
        before = set(self.root.iterdir())
        result = subprocess.run([sys.executable, "-B", bulk.__file__, "--db", str(catalog),
                                 "--volume-uuid", "wrong-volume"], capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("must be on volume wrong-volume", result.stderr)
        self.assertEqual(set(self.root.iterdir()), before)

    def test_unmounted_removable_path_rejected(self):
        with patch.object(Path, "is_mount", return_value=False), self.assertRaisesRegex(OSError, "not mounted"):
            bulk.catalog.check_removable_mount(Path("/run/media/m/Minkowski/Data/Lidar/catalog.sqlite"))
