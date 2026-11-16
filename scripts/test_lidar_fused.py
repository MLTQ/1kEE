import json
from pathlib import Path
import sqlite3
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import laspy
import numpy as np
from pyproj import CRS
import rasterio

import lidar_catalog as catalog
import lidar_fused as fused
import lidar_map as maps


class FusedTests(unittest.TestCase):
    def setUp(self):
        self.folder = tempfile.TemporaryDirectory()
        self.addCleanup(self.folder.cleanup)
        self.root = Path(self.folder.name)
        index = self.root/'fused/v1/index.sqlite'
        index.parent.mkdir(parents=True)
        self.index = sqlite3.connect(index)
        self.addCleanup(self.index.close)
        self.index.executescript(fused.SCHEMA)
        self.catalog = sqlite3.connect(':memory:')
        self.addCleanup(self.catalog.close)
        self.catalog.row_factory = sqlite3.Row
        self.catalog.executescript(catalog.SCHEMA)
        self.box = maps.tile_bounds(15,18640,9549)

    def scan(self, name, heights, date='2026-01-01', columns=(100,100,300), classes=None, withheld=None):
        w,s,e,n = self.box
        step=(e-w)/maps.SIZE
        header=laspy.LasHeader(point_format=3,version='1.2')
        header.scales=np.array([1e-9,1e-9,.001])
        header.offsets=np.array([w,s,0.])
        header.add_crs(CRS.from_epsg(4326))
        cloud=laspy.LasData(header)
        cloud.x=np.array([w+(col+.25)*step for col in columns])
        cloud.y=np.array([n-(100.25+i%2)*step for i in range(len(heights))])
        cloud.z=np.array(heights,dtype=float)
        if classes is not None:
            cloud.classification=classes
        if withheld is not None:
            cloud.withheld=withheld
        path=self.root/(name+'.las')
        cloud.write(path)
        return dict(url='https://example.org/'+name+'.laz',survey='NOAAID:'+name,
                    path=path,date=date)

    def add_catalog(self, asset):
        geom=catalog.polygon_geometry([catalog.rectangle(*self.box)])
        row=catalog.record(asset['survey'],asset['survey'],'lidar',asset['date'],
                           'https://example.org',geom,asset['url'])
        self.catalog.execute('INSERT INTO surveys VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?)',
                             (asset['survey'],'opentopography',*row[1:]))

    def test_full_density_statistics_match_independent_point_reduction(self):
        asset=self.scan('a',[10,20,30,900,800],columns=(100,100,300,300,300),
                        classes=[2,2,2,7,2],withheld=[0,0,0,0,1])
        self.assertEqual(fused.prepare(self.root,self.index,asset),'built')
        row=self.index.execute('SELECT path,points,valid_points,valid_cells,source_sha256 FROM rasters').fetchone()
        self.assertEqual(row[1:3],(5,3))
        self.assertEqual(row[4],fused.digest(asset['path']))
        with rasterio.open(self.root/row[0]) as ds:
            bands=ds.read()
            self.assertEqual(ds.descriptions,('min','max','mean','count'))
            # Independently group original points using rasterio's geographic lookup.
            groups={}
            with laspy.open(asset['path']) as reader:
                points=reader.read()
                for i in range(3):
                    cell=ds.index(float(points.x[i]),float(points.y[i]))
                    groups.setdefault(cell,[]).append(float(points.z[i]))
            self.assertEqual(len(groups),row[3])
            for (y,x),values in groups.items():
                np.testing.assert_allclose(bands[:,y,x],[min(values),max(values),np.mean(values),len(values)],atol=1e-4)
            self.assertEqual(np.isfinite(bands[2]).sum(),len(groups))

    def test_resume_repairs_corruption_and_revises_dates(self):
        asset=self.scan('a',[10,20,30])
        fused.prepare(self.root,self.index,asset)
        original=self.index.execute('SELECT id,path FROM rasters').fetchone()
        self.assertEqual(fused.prepare(self.root,self.index,asset),'reused')
        (self.root/original[1]).write_bytes(b'broken')
        self.assertEqual(len(fused.verify(self.root)['failures']),1)
        self.assertEqual(fused.prepare(self.root,self.index,asset),'built')
        self.assertEqual(fused.verify(self.root)['verified'],1)
        asset['date']='2026-03-01'
        self.assertEqual(fused.prepare(self.root,self.index,asset),'built')
        new=self.index.execute('SELECT id,date FROM rasters').fetchone()
        self.assertGreater(new[0],original[0])
        self.assertEqual(new[1],asset['date'])

    def test_failed_conversion_never_publishes_or_removes_raw(self):
        asset=self.scan('a',[10,20,30])
        with patch.object(fused,'python_build',side_effect=ValueError('decode failed')):
            with self.assertRaisesRegex(ValueError,'decode failed'):
                fused.prepare(self.root,self.index,asset)
        self.assertTrue(asset['path'].is_file())
        self.assertEqual(self.index.execute('SELECT count(*) FROM rasters').fetchone(),(0,))
        self.assertFalse(list((self.root/'fused/v1/rasters').glob('*.tif')))

    def test_latest_measured_pixels_win_and_older_data_fills_only_gaps_offline(self):
        old=self.scan('old',[10,10,10],date='2020-01-01')
        new=self.scan('new',[20,20],date='2026-01-01',columns=(100,100))
        for asset in [old,new]:
            self.add_catalog(asset)
            fused.prepare(self.root,self.index,asset)
            asset['path'].unlink()
        with patch.object(maps,'survey_image',side_effect=AssertionError('network requested')):
            image,credits=maps.measurements(self.catalog,self.box,self.root/'map-source-cache','')
        self.assertEqual([c[1] for c in credits],[new['survey'],old['survey']])
        self.assertEqual(image.getpixel((100,100)),maps.elevation_image(np.full((512,512),20.),self.box).getpixel((100,100)))
        self.assertGreater(image.getpixel((300,100))[3],0)
        self.assertEqual(image.getpixel((200,100))[3],0)
        # An explicit historical acquisition remains available from numeric data.
        image,credits=maps.measurements(self.catalog,self.box,self.root/'map-source-cache',old['survey'])
        self.assertEqual([c[1] for c in credits],[old['survey']])
        self.assertGreater(image.getpixel((100,100))[3],0)

    def test_missing_raster_is_not_reported_as_prepared(self):
        asset=self.scan('a',[10,20,30])
        fused.prepare(self.root,self.index,asset)
        (self.root/self.index.execute('SELECT path FROM rasters').fetchone()[0]).unlink()
        self.assertEqual(maps.fused_assets(self.root,self.box),[])

    def test_cli_pins_volume_and_stores_reusable_relative_receipts(self):
        asset=self.scan('a',[10,20,30])
        with sqlite3.connect(self.root/'catalog.sqlite') as db:
            db.executescript(catalog.SCHEMA)
        with sqlite3.connect(self.root/'bulk.sqlite') as db:
            db.execute('CREATE TABLE files (url TEXT,survey TEXT,path TEXT,bytes INTEGER,kind TEXT)')
            db.execute('INSERT INTO files VALUES(?,?,?,?,?)',
                       (asset['url'],asset['survey'],asset['path'].name,asset['path'].stat().st_size,'copc'))
        command=[sys.executable,str(Path(fused.__file__).resolve()),'--root',str(self.root),'--max-files','1']
        run=subprocess.run(command,capture_output=True,text=True)
        self.assertEqual(run.returncode,0,run.stderr)
        row=self.index.execute('SELECT source_path,path FROM rasters').fetchone()
        self.assertEqual(row[0],asset['path'].name)
        self.assertFalse(Path(row[1]).is_absolute())
        self.assertTrue((self.root/row[1]).is_file())
        run=subprocess.run(command,capture_output=True,text=True)
        self.assertEqual(json.loads(run.stdout)['status'],'reused')
        run=subprocess.run(command+['--volume-uuid','incorrect-volume'],capture_output=True,text=True)
        self.assertNotEqual(run.returncode,0)
        self.assertIn('identity mismatch',run.stderr)


if __name__=='__main__':
    unittest.main()
