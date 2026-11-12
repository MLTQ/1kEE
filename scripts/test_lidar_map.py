import json
from pathlib import Path
import sqlite3
import tempfile
import unittest
from unittest.mock import patch

import numpy as np
from PIL import Image

import lidar_catalog as catalog
import lidar_map as maps


class LidarMapTests(unittest.TestCase):
    def setUp(self):
        self.db = sqlite3.connect(":memory:")
        self.addCleanup(self.db.close)
        self.db.row_factory = sqlite3.Row
        self.db.executescript(catalog.SCHEMA)
        self.box = maps.tile_bounds(12, 2387, 1175)

    def add(self, key, source, bounds, asset=None):
        geometry = catalog.polygon_geometry([catalog.rectangle(*bounds)])
        row = catalog.record(key, key, "lidar", None, "https://example.org", geometry, asset)
        self.db.execute("INSERT INTO surveys VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?)", (key, source, *row[1:]))

    def test_coverage_alone_never_produces_measurements(self):
        self.add("archive", "eba", self.box)
        self.assertIsNotNone(maps.paths(self.db, self.box).getbbox())
        with patch.object(maps, "survey_image", side_effect=AssertionError("archive fetch")):
            relief, sources = maps.measurements(self.db, self.box, Path("."), "")
        self.assertIsNone(relief.getbbox())
        self.assertFalse(sources)

    def test_selected_survey_and_unmeasured_pixels_survive_png_roundtrip(self):
        self.add("NOAAID:38", "opentopography", self.box)
        self.add("NOAAID:42", "opentopography", self.box)
        values = np.full((maps.SIZE, maps.SIZE), np.nan)
        values[17:27, 40:47] = np.arange(10)[:, None] - 8
        with patch.object(maps, "survey_image", return_value=maps.elevation_image(values, self.box)) as source:
            image, credits = maps.measurements(self.db, self.box, Path("."), "NOAAID:38")
        self.assertEqual(source.call_count, 1)
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder, "map.png")
            maps.save(image, path, credits)
            with Image.open(path) as cached:
                self.assertEqual(json.loads(cached.info["Surveys"])[0][1], "NOAAID:38")
                self.assertEqual(cached.getchannel("A").getbbox(), (40, 17, 47, 27))
                self.assertNotEqual(cached.getpixel((40, 17)), cached.getpixel((40, 26)))

    def test_source_failure_retries_and_other_published_sources_can_fill_a_tile(self):
        self.add("NOAAID:1", "opentopography", self.box)
        self.add("NOAAID:2", "opentopography", self.box)
        with patch.object(maps, "survey_image", side_effect=OSError("offline")), self.assertRaises(ValueError):
            maps.measurements(self.db, self.box, Path("."), "")
        valid = Image.new("RGBA", (maps.SIZE, maps.SIZE), (100, 110, 120, 255))
        with patch.object(maps, "survey_image", side_effect=[OSError("unavailable"), valid]):
            result, credits = maps.measurements(self.db, self.box, Path("."), "")
        self.assertIsNotNone(result.getbbox())
        self.assertEqual(credits[0][1], "NOAAID:2")

    def test_gap_in_selected_year_stays_empty_when_another_year_covers_it(self):
        self.add("NOAAID:38", "opentopography", self.box)
        self.add("NOAAID:42", "opentopography", self.box)
        with patch.object(maps, "survey_image", return_value=None) as get:
            image, credits = maps.measurements(self.db, self.box, Path("."), "NOAAID:38")
        self.assertEqual(get.call_count, 1)
        self.assertIsNone(image.getbbox())
        self.assertEqual(credits, [])

    def test_linz_reuses_unchanged_metadata_and_rejects_invalid_checksum(self):
        import hashlib
        body = b'{"type":"Collection"}'
        link = {"href": "one.json", "file:checksum": "1220" + hashlib.sha256(body).hexdigest()}
        with patch.object(catalog, "request", return_value=(body, {})) as get:
            for _ in range(2):
                _, item = catalog.linz_document(self.db, "https://tiles.test/root.json", link)
                self.assertEqual(item["type"], "Collection")
            self.assertEqual(get.call_count, 1)
            with self.assertRaises(ValueError):
                catalog.linz_document(self.db, "https://tiles.test/root.json", dict(link, **{"file:checksum":"wrong"}))
