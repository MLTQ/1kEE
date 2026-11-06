"""Scientific-value, row-order and publication regressions for NCEI preparation."""
import array
import json
import pathlib
import shutil
import sys
import tempfile
import unittest

import fetch_ncei_gravity as gravity


class PreparationTests(unittest.TestCase):
    def spec(self):
        return dict(id='fixture', kind='free_air', label='Fixture', note='Fixture',
                    spacing='1 degree', url='https://example.test/source.bin',
                    notes_url='https://example.test/notes', crs_note='Fixture CRS',
                    size=[3, 2], type='i', scale=.001, missing=99999,
                    row_order='south_to_north', geotransform=[-120, 1, 0, 42, 0, -1],
                    srs='EPSG:4326', target_spacing=None)

    def source(self, folder):
        # South row first. Zero is valid; the third southern cell is missing.
        values = array.array('i', [-12345, 0, 99999000, 1234, 5678, 9876])
        if sys.byteorder != 'little':
            values.byteswap()
        path = folder / 'source.bin'
        path.write_bytes(values.tobytes())
        return path

    def test_scale_north_up_and_missing_are_independent(self):
        with tempfile.TemporaryDirectory() as folder:
            folder = pathlib.Path(folder)
            output = folder / 'physical.bin'
            stats = gravity.physical_grid(self.source(folder), self.spec(), output)
            values = array.array('f', output.read_bytes())
            if sys.byteorder != 'little':
                values.byteswap()
            for actual, expected in zip(values, [1.234, 5.678, 9.876, -12.345, 0, gravity.NODATA]):
                self.assertAlmostEqual(actual, expected, places=5)
            self.assertEqual(stats['missing'], 1)
            self.assertEqual(stats['valid'], 5)

    def test_float_missing_rounding_and_truncation(self):
        with tempfile.TemporaryDirectory() as folder:
            folder = pathlib.Path(folder)
            spec = self.spec()
            spec.update(size=[2, 1], type='f', scale=1, missing=9999.99)
            data = array.array('f', [9999.99, -45.125])
            if sys.byteorder != 'little':
                data.byteswap()
            source = folder / 'source.bin'
            source.write_bytes(data.tobytes())
            stats = gravity.physical_grid(source, spec, folder / 'out')
            self.assertEqual(stats['missing'], 1)
            source.write_bytes(b'bad')
            with self.assertRaisesRegex(ValueError, 'length'):
                gravity.physical_grid(source, spec, folder / 'out')

    @unittest.skipUnless(shutil.which('gdalinfo'), 'GDAL required')
    def test_geographic_samples_and_atomic_publication(self):
        with tempfile.TemporaryDirectory() as folder:
            folder = pathlib.Path(folder)
            output = folder / 'prepared'
            source = self.source(folder)
            manifest = gravity.prepare(source, self.spec(), output)
            self.assertEqual(manifest['size'], [3, 2])
            self.assertEqual(manifest['bounds'], [-120, 40, -117, 42])
            self.assertEqual(manifest['units'], 'mGal')
            self.assertEqual(manifest['grid_sha256'], gravity.digest(output / 'grid.bil'))
            values = array.array('f', (output / 'grid.bil').read_bytes())
            if sys.byteorder != 'little':
                values.byteswap()
            self.assertAlmostEqual(values[0], 1.234, places=5)
            self.assertEqual(values[-1], gravity.NODATA)
            with self.assertRaises(FileExistsError):
                gravity.prepare(source, self.spec(), output)

    def test_catalog_has_only_selected_quantities_and_safe_unique_ids(self):
        specs = json.loads(gravity.CATALOG.read_text())
        self.assertEqual({s['kind'] for s in specs}, {'free_air', 'bouguer', 'isostatic'})
        self.assertEqual(len({s['id'] for s in specs}), len(specs))
        for spec in specs:
            self.assertTrue(spec['id'].replace('_', '').isalnum())
            self.assertEqual(len(spec['sha256']), 64)
            w, h = spec['size']
            self.assertEqual(spec['bytes'], w * h * {'h': 2, 'i': 4, 'f': 4}[spec['type']])
        iso = next(s for s in specs if s['id'] == 'isores')
        self.assertEqual(iso['missing'], 999999.99)
        seg = next(s for s in specs if s['id'] == 'seggrav')
        self.assertIn('offshore', seg['note'])


if __name__ == '__main__':
    unittest.main()
