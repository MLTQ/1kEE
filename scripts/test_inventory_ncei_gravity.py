"""Small protocol fixtures for a metadata-only NCEI byte inventory."""
import unittest
import pathlib
import tempfile
from types import SimpleNamespace
from unittest.mock import patch
from inventory_ncei_gravity import catalog_products, listing, listed_bytes, numeric_key, summarize
from inventory_ncei_gravity import Metadata
from ncei_gravity_maps import PREFIX, category, map_summary


class InventoryTests(unittest.TestCase):
    def test_catalog_shared_access_roots_and_apache_scope(self):
        html = '<table><tr><td>Nevada Bouguer Grid</td><td>U.S.</td><td><a href="https://www.ngdc.noaa.gov/mgg/gravity/1999/data/grids/eros/nevada/">Access</a></td></tr></table>'
        product = catalog_products(html)[0]
        self.assertEqual(product['name'], 'Nevada Bouguer Grid')
        root = product['url']
        html = '''<table><tr><td><a href="../">Parent Directory</a></td><td></td><td>-</td></tr>
          <tr><td><a href="?C=N;O=D">Name</a></td><td></td><td></td></tr>
          <tr><td><a href="nevboug.bin">nevboug.bin</a></td><td>2008-02-28</td><td>437K</td></tr>
          <tr><td><a href="lists/">lists/</a></td><td>2008</td><td>-</td></tr></table>'''
        rows = listing(html, root)
        self.assertEqual([r['name'] for r in rows], ['nevboug.bin', 'lists/'])
        self.assertEqual(rows[0]['url'], root + 'nevboug.bin')
        self.assertEqual(listed_bytes('1.8M'), round(1.8 * 1024 ** 2))
        self.assertIsNone(listed_bytes('-'))

    def test_alternate_formats_count_once_and_unknown_sizes_stay_unknown(self):
        files = [{'url': 'https://example.org/a.bin', 'bytes': 100},
                 {'url': 'https://example.org/a.xyz', 'bytes': 400},
                 {'url': 'https://example.org/b.grd.gz', 'bytes': 60},
                 {'url': 'https://example.org/readme.txt', 'bytes': 10},
                 {'url': 'https://example.org/c.grd', 'bytes': None}]
        for row in files:
            row['numeric_key'] = numeric_key(row['url'])
        totals = summarize(files)
        self.assertEqual(totals['known_bytes_all_files'], 570)
        self.assertEqual(totals['known_bytes_all_numeric_formats'], 560)
        self.assertEqual(totals['smallest_representation_bytes'], 160)
        self.assertEqual(totals['named_numeric_products'], 3)
        self.assertEqual(totals['exact_sized_files'], 4)
        self.assertEqual(totals['numeric_products_missing_all_sizes'], ['/c'])

    def test_head_uses_final_redirect_size_and_reuses_metadata_cache(self):
        response = b'HTTP/1.1 301 Moved\r\nContent-Length: 0\r\n\r\nHTTP/2 200\r\nContent-Length: 4096\r\nContent-Type: application/octet-stream\r\n\r\n'
        with tempfile.TemporaryDirectory() as folder:
            client = Metadata(pathlib.Path(folder), interval=0)
            with patch('inventory_ncei_gravity.subprocess.run', return_value=SimpleNamespace(stdout=response)) as request:
                first = client.fetch('https://example.org/gravity.bin', head=True)
                second = client.fetch('https://example.org/gravity.bin', head=True)
                self.assertEqual(first['bytes'], 4096)
                self.assertEqual(first, second)
                self.assertEqual(request.call_count, 1)
                self.assertIn('-I', request.call_args.args[0])

    def test_header_types_override_ambiguous_catalog_names(self):
        self.assertEqual(category('/grids/nevdepth', 'nevada depth-to-basement grid', 'image'),
                         'terrain_or_other_reference_maps')
        self.assertEqual(category('/grids/fsu', 'FSU Gravity Data', 'point'), 'observation_tables')
        self.assertEqual(category('/grids/grav96us', 'terrain corrected free-air anomalies (like geoid96)', 'image'),
                         'gravity_anomaly_maps')
        self.assertEqual(category('/grids/n96us', 'gravity data per 2 min cell', 'image'), 'sampling_density_maps')

    def test_mirror_and_tiny_ascii_do_not_replace_the_original_binary_map(self):
        native = 'https://www.ngdc.noaa.gov' + PREFIX + 'grids/dgrav/dgrav'
        mirror = 'https://www.ngdc.noaa.gov' + PREFIX + 'grids/idrisi/dgrav'
        files = [{'url': native + '.bin', 'bytes': 15693696},
                 {'url': native + '.xyz', 'bytes': 35},
                 {'url': mirror + '.img', 'bytes': 49904}]
        for item in files:
            item['numeric_key'] = numeric_key(item['url'])
        metadata = [{'url': native + '.hdr', 'text': 'file_title = North America gravity anomaly grid\ndata_type = image\n'}]
        result = map_summary(files, metadata)
        maps = result['categories']['gravity_anomaly_maps']
        self.assertEqual(maps['products'], 1)
        self.assertEqual(maps['numeric_files'], 3)
        self.assertEqual(maps['preferred_format_bytes'], 15693696)
        self.assertEqual(result['placeholder_sized_numeric_files'], [native + '.xyz'])

    def test_ocean_readme_identifies_compressed_geographic_grid(self):
        root = 'https://www.ngdc.noaa.gov/mgg/gravity/2008/data/'
        files = [{'url': root + 'grav_down.15.2.img.gz', 'bytes': 80},
                 {'url': root + 'grav_down.15.2.nc.gz', 'bytes': 83}]
        for item in files:
            item['numeric_key'] = numeric_key(item['url'])
        metadata = [{'url': root + 'readme.txt',
                     'text': 'grav_down.15.2.img\tDownward continued gravity.\n'}]
        p = map_summary(files, metadata)['products'][0]
        self.assertEqual(p['title'], 'Downward continued gravity.')
        self.assertEqual(p['category'], 'gravity_anomaly_maps')
        self.assertTrue(p['preferred_url'].endswith('.nc.gz'))


if __name__ == '__main__':
    unittest.main()
