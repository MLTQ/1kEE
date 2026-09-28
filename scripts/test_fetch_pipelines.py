import io
import json
import unittest
from fetch_pipelines import normalize


class PipelineNormalization(unittest.TestCase):
    def test_multipart_and_altitude_do_not_create_connections_or_fake_heights(self):
        out = io.StringIO()
        feature = {"properties": {"project-id": "P3940", "status": "proposed", "owner": "Owner"}, "geometry": {"type": "MultiLineString", "coordinates": [[[1, 2, 0], [3, 4, 7]], [[10, 20], [30, 40]]]}}
        stats = normalize("gem-gas", [feature], out)
        rows = [json.loads(line) for line in out.getvalue().splitlines()]
        self.assertEqual(len(rows), 2)
        self.assertEqual(rows[0]["points"], [[1, 2], [3, 4]])
        self.assertEqual(rows[1]["points"], [[10, 20], [30, 40]])
        self.assertTrue(rows[0]["info"]["planned"])
        self.assertEqual(rows[0]["info"]["operator"], "")
        self.assertEqual(stats["extra_ordinates_omitted"], 2)

    def test_offshore_umbilicals_excluded_and_retired_fuel_preserved(self):
        out = io.StringIO()
        def feature(product, status):
            return {"properties": {"PROD_CODE": product, "STATUS_COD": status}, "geometry": {"type": "LineString", "coordinates": [[-90, 28], [-90.1, 28.1]]}}
        stats = normalize("bsee", [feature("UMB", "ACT"), feature("OIL", "ABN")], out)
        rows = [json.loads(line) for line in out.getvalue().splitlines()]
        self.assertEqual(len(rows), 1)
        self.assertTrue(rows[0]["info"]["historical"])
        self.assertEqual(rows[0]["info"]["product"], "oil")
        self.assertEqual(stats["non_fuel_records"], 1)


if __name__ == "__main__":
    unittest.main()
