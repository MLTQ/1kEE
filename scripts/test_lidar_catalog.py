import argparse
import io
import json
from pathlib import Path
import sqlite3
import tempfile
import unittest
from unittest.mock import patch
from urllib.error import HTTPError

import lidar_catalog as lidar


class LidarCatalogTests(unittest.TestCase):
    def test_repeated_ids_keep_both_scan_footprints_and_skip_photogrammetry(self):
        def dataset(name, x, uid):
            geometry = lidar.polygon_geometry([lidar.rectangle(x, 0, x + 1, 1)])
            return {"Dataset": {"name": name, "identifier": {"propertyID": "opentopoID", "value": uid},
                                "url": "https://doi.org/example", "spatialCoverage": {"geo": {"geojson": {
                                    "features": [{"geometry": geometry}]}}}}}
        sfm = dataset("Photogrammetry survey", 6, "B")
        sfm["Dataset"]["description"] = "A photogrammetry-derived set of lidar files"
        rows = list(lidar.opentopography({"Datasets": [
            dataset("Test lidar", 1, "A"), dataset("Test lidar", 3, "A"),
            dataset("Test lidar", 1, "A"), sfm]}, {"links": []}))
        self.assertEqual(len(rows), 1)
        self.assertEqual(rows[0][-4:], (1, 0, 4, 1))
        self.assertEqual(len(json.loads(rows[0][7])["coordinates"]), 2)

    def test_nasa_pagination_asset_filter_and_coordinates(self):
        def entry(key, **spatial):
            return {"id": key, **spatial, "links": [
                {"href": f"https://example.org/{key}.laz", "rel": "urn/data#"},
                {"href": "https://example.org/guide.pdf", "rel": "urn/metadata#"}]}
        p1 = [entry("G1", polygons=[["1 -59 2 -58 1 -58"]])]
        p2 = [entry("G2", points=["-3.3347 -47.6996"])]
        with patch.object(lidar, "cmr_page", side_effect=[
            (p1, {"CMR-Hits": "2", "CMR-Search-After": "abc"}), (p2, {"CMR-Hits": "2"})]) as get:
            rows = list(lidar.cmr_records("C1", 2))
        self.assertEqual(get.call_args.args[-1], {"CMR-Search-After": "abc"})
        self.assertEqual(rows[0][-4:], (-59, 1, -58, 2))
        self.assertEqual(json.loads(rows[1][7]), {"type": "Point", "coordinates": [-47.6996, -3.3347]})

    def test_failed_refresh_preserves_previous_snapshot_and_daily_check_skips_http(self):
        db = sqlite3.connect(":memory:")
        db.executescript(lidar.SCHEMA)
        good = lidar.record("S", "scan", "point cloud", None, "https://example.org",
                            lidar.polygon_geometry([lidar.rectangle(-60, -5, -59, -4)]))
        with patch.object(lidar, "cmr_version", return_value="1:old"), \
             patch.object(lidar, "cmr_records", return_value=[good]):
            lidar.refresh_source(db, "nasa-brazil", False, lidar.COLLECTIONS)
        with patch.object(lidar, "cmr_version", side_effect=AssertionError("unnecessary network")):
            self.assertIn("24h", lidar.refresh_source(db, "nasa-brazil", False, lidar.COLLECTIONS))

        def partial(*_args):
            yield good
            raise ValueError("incomplete")
        with patch.object(lidar, "cmr_version", return_value="2:new"), \
             patch.object(lidar, "cmr_records", partial), self.assertRaises(ValueError):
            lidar.refresh_source(db, "nasa-brazil", True, lidar.COLLECTIONS)
        self.assertEqual(db.execute("select id from surveys").fetchall(), [("S",)])
        self.assertEqual(db.execute("select version from refreshes").fetchone(), ("1:old",))

    def test_http_304_uses_existing_response(self):
        db = sqlite3.connect(":memory:")
        db.executescript(lidar.SCHEMA)
        with patch.object(lidar, "request", return_value=(b"snapshot", {"ETag": '"one"'})):
            lidar.cached(db, "https://example.org")
        with patch.object(lidar, "request", side_effect=HTTPError("url", 304, "", {}, io.BytesIO())) as get:
            self.assertEqual(lidar.cached(db, "https://example.org"), b"snapshot")
            self.assertEqual(get.call_args.args[1], {"If-None-Match": '"one"'})

    def test_stac_pagination_keeps_scan_links_and_rejects_cycles(self):
        url = "https://surveys.test/items"
        item = {"id": "SW1", "properties": {}, "links": [{"rel": "self", "href": "/SW1"}],
                "geometry": lidar.polygon_geometry([lidar.rectangle(5, 40, 6, 41)]),
                "assets": {"cloud": {"href": "/point.las.zip"}, "preview": {"href": "/preview.jpg"}}}
        page = {"type": "FeatureCollection", "features": [item],
                "links": [{"rel": "next", "href": "?after=1"}]}
        end = {"type": "FeatureCollection", "features": [], "links": []}
        with patch.object(lidar, "cached", side_effect=[json.dumps(page), json.dumps(end)]) as get:
            rows = list(lidar.stac_records(None, "test", url))
        self.assertEqual(get.call_args.args[1], url + "?after=1")
        self.assertEqual(len(rows), 1)
        self.assertEqual(rows[0][5], "https://surveys.test/point.las.zip")
        with patch.object(lidar, "cached", return_value=json.dumps(page)), self.assertRaises(ValueError):
            list(lidar.stac_records(None, "test", url))

    def test_terrain_reads_only_overlapping_bare_earth_assets(self):
        coll = {"links": [{"rel": "item", "href": "https://survey.test/s_be.json"},
                          {"rel": "item", "href": "https://survey.test/s_hh.json"}]}
        def asset(coords, name):
            return {"bbox": coords, "href": "https://test/" + name,
                    "roles": ["data"], "type": "image/tiff; profile=cloud-optimized"}
        item = {"assets": {"inside": asset([-61, -4, -59, -2], "near.tif"),
                           "outside": asset([20, 30, 21, 31], "far.tif")}}
        with patch.object(lidar, "request", side_effect=[(json.dumps(coll), {}), (json.dumps(item), {})]):
            self.assertEqual(lidar.ground_assets("https://survey.test", -60, -3, 2),
                             ["/vsicurl/https://test/near.tif"])

    def test_failed_download_never_publishes_login_page_and_success_skips_repeat(self):
        with tempfile.TemporaryDirectory() as tmp:
            args = argparse.Namespace(id="scan", source=None, bbox=None, all=False, eba_archives=False,
                                      max_files=1, list=False, netrc=False, directory=Path(tmp))
            url = "https://example.org/scan.laz"
            with patch.object(lidar, "scan_files", return_value=[(url, "scan.laz", None, None)]):
                def send(body):
                    def write(cmd, **_kwargs):
                        Path(cmd[cmd.index("--output") + 1]).write_bytes(body)
                        return lidar.subprocess.CompletedProcess(cmd, 0, stderr="")
                    return write
                with patch.object(lidar.subprocess, "run", send(b"<html>login")), self.assertRaises(ValueError):
                    lidar.download(None, args)
                self.assertFalse(list(Path(tmp).rglob("*.laz")))
                with patch.object(lidar.subprocess, "run", send(b"LASFdata")):
                    lidar.download(None, args)
                with patch.object(lidar.subprocess, "run", side_effect=AssertionError("download repeated")):
                    lidar.download(None, args)


if __name__ == "__main__":
    unittest.main()
