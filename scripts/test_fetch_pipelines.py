import io
import json
import unittest
from fetch_pipelines import capacity, normalize


class PipelineNormalization(unittest.TestCase):
    def test_capacity_retains_units_release_and_zero_without_using_display_scaling(self):
        for source, unit, value in [("gem-gas", "bcm/y of gas", 12.5), ("gem-oil", "boe/d", 120000), ("gem-gas", "bcm/y of gas", 0)]:
            with self.subTest(source=source, value=value):
                out = io.StringIO()
                feature = {"properties": {"project-id": "P1", "fuel": "Oil", "capacity": value, "units-of-m": unit, "capacity-scaled": 999999}, "geometry": {"type": "MultiLineString", "coordinates": [[[1, 2], [3, 4]], [[5, 6], [7, 8]]]}}
                stats = normalize(source, [feature], out)
                rows = [json.loads(line) for line in out.getvalue().splitlines()]
                report = rows[0]["info"]["capacity"]
                self.assertEqual(report["value"], value)
                self.assertEqual(report["unit"], unit)
                self.assertIn("2025" if source == "gem-gas" else "2026", report["source_release"])
                self.assertEqual(rows[1]["info"]["capacity"], report)
                self.assertEqual(stats["with_capacity"], 1)  # Source features, not tiled fragments.

    def test_unknown_invalid_and_non_gem_capacity_stays_unavailable(self):
        for value in [None, "", "unknown", "12.5", True, -1, float("nan"), float("inf")]:
            with self.subTest(value=value):
                self.assertIsNone(capacity("gem-gas", {"capacity": value, "units-of-m": "bcm/y of gas", "capacity-scaled": 0}))
        self.assertIsNone(capacity("gem-gas", {"capacity": 4}))
        self.assertIsNone(capacity("gem-gas", {"capacity": 4, "units-of-m": " "}))
        self.assertIsNone(capacity("eia-3", {"capacity": 4, "units-of-m": "bcm/y of gas"}))

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
