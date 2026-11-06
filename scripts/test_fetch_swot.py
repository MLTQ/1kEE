"""Exercise real GDAL conversion, scaling, registration and missing-data handling."""
import json
import pathlib
import shutil
import struct
import tempfile
import unittest

from fetch_swot import prepare, validate_source


@unittest.skipUnless(all(shutil.which(t) for t in ['gdal_translate', 'gdalwarp', 'gdalinfo']), 'GDAL required')
class PreparationTests(unittest.TestCase):
    def test_physical_values_missing_cells_polar_caps_and_provenance(self):
        with tempfile.TemporaryDirectory() as folder:
            root = pathlib.Path(folder)
            values = [1.2, 2.4, float('nan'), 3.4, -20, 0, 20, 100,
                      -1.2, -2.4, -3.4, float('nan'), 20, 0, -20, -100]
            (root / 'samples.raw').write_bytes(struct.pack('<16f', *values))
            source = root / 'source.vrt'
            source.write_text('''<VRTDataset rasterXSize="8" rasterYSize="2">
              <SRS>EPSG:4326</SRS><GeoTransform>-180,45,0,45,0,-45</GeoTransform>
              <VRTRasterBand dataType="Float32" band="1" subClass="VRTRawRasterBand">
                <NoDataValue>nan</NoDataValue><SourceFilename relativeToVRT="1">samples.raw</SourceFilename>
                <ImageOffset>0</ImageOffset><PixelOffset>4</PixelOffset><LineOffset>32</LineOffset><ByteOrder>LSB</ByteOrder>
              </VRTRasterBand></VRTDataset>''')
            output = root / 'prepared'
            prepare(source, output, native=(8, 4), overview=(4, 2))
            actual = struct.unpack('<32h', (output / 'vgg_8x4.bil').read_bytes())
            self.assertEqual(actual[:8], (-32768,) * 8)
            self.assertEqual(actual[8:16], (12, 24, -32768, 34, -200, 0, 200, 1000))
            self.assertEqual(actual[16:24], (-12, -24, -34, -32768, 200, 0, -200, -1000))
            self.assertEqual(actual[24:], (-32768,) * 8)
            overview = struct.unpack('<8h', (output / 'vgg_4x2.bil').read_bytes())
            self.assertEqual(overview, (18, 34, -100, 600, -18, -34, 100, -600))
            manifest = json.loads((output / 'manifest.json').read_text())
            self.assertEqual(manifest['scale'], 0.1)
            self.assertEqual(len(manifest['source_sha256']), 64)
            with self.assertRaises(FileExistsError):
                prepare(source, output, native=(8, 4), overview=(4, 2))


class ValidationTests(unittest.TestCase):
    def test_rejects_wrong_grid_or_packed_units(self):
        metadata = {'size': [21600, 9600], 'geoTransform': [-180, 1 / 60, 0, 80, 0, -1 / 60],
                    'bands': [{'type': 'Float32'}]}
        validate_source(metadata)
        metadata['bands'][0]['scale'] = 0.1
        with self.assertRaises(ValueError):
            validate_source(metadata)
        metadata['bands'][0].pop('scale')
        metadata['geoTransform'][0] = 0
        with self.assertRaises(ValueError):
            validate_source(metadata)


if __name__ == '__main__':
    unittest.main()
