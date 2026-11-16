#!/usr/bin/env python3
"""Compare the native converter/renderer with the frozen Python benchmark oracle.

Reads original scans; generated rasters, SQLite fixtures and PNGs stay in --output.
"""
import argparse
import hashlib
import json
from pathlib import Path
import sqlite3
import statistics
import subprocess
import time

import numpy as np
from PIL import Image
import rasterio

from benchmark_lidar_fusion import grid, python_build, read_fused
import lidar_map as maps


def sha(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream,'sha256').hexdigest()


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--manifest',type=Path,required=True)
    p.add_argument('--native',type=Path,required=True)
    p.add_argument('--output',type=Path,required=True)
    p.add_argument('--runs',type=int,default=3)
    args=p.parse_args()
    args.output.mkdir(parents=True,exist_ok=True)
    report=dict(protocol='One warmup and three alternating pairs. OS/drive caches uncontrolled. '
        'Native timings include executable startup and native library loading; Python functions '
        'run in the warmed harness. Native windowed TIFF reads are measured on the Python-written '
        'v1 archive to prove compatibility. Numeric equivalence includes all four bands.',
        native_sha256=sha(args.native),runs=args.runs,fixtures=[])
    for fixture in json.loads(args.manifest.read_text()):
        source=Path(fixture['path']);out=args.output/fixture['name'];out.mkdir(exist_ok=True)
        spec=grid(source)
        paths={engine:out/(engine+'.tif') for engine in ['python','native']}
        def native():
            run=subprocess.run([str(args.native),'convert','--input',str(source),'--output',str(paths['native'])],
                               capture_output=True,text=True,timeout=600)
            if run.returncode:raise RuntimeError(run.stderr)
            return json.loads(run.stdout)
        actions={'python':lambda:python_build(source,paths['python'],spec),'native':native}
        row=dict(fixture,sha256=sha(source),builds=[],reads=[])
        for repeat in range(-1,args.runs):
            for engine in (['python','native'] if repeat%2 else ['native','python']):
                start=time.perf_counter();stats=actions[engine]();elapsed=time.perf_counter()-start
                row['builds'].append(dict(repeat=repeat,engine=engine,seconds=elapsed))
                if engine=='native':row['native_stats']=stats
                print(f'{fixture["name"]} {engine} {repeat}: {elapsed:.3f}s',flush=True)
        with rasterio.open(paths['python']) as a,rasterio.open(paths['native']) as b:
            aa,bb=a.read(),b.read();valid=np.isfinite(aa[2])&np.isfinite(bb[2])
            errors=np.abs(aa[2][valid]-bb[2][valid])
            row['parity']=dict(grid_identical=a.transform==b.transform and a.shape==b.shape,
                coverage_mismatch=int(np.count_nonzero(np.isfinite(aa[2])!=np.isfinite(bb[2]))),
                count_mismatch=int(np.count_nonzero(np.nan_to_num(aa[3])!=np.nan_to_num(bb[3]))),
                max_mean_error=float(errors.max(initial=0)),
                max_extrema_error=float(np.nanmax(np.abs(aa[:2]-bb[:2]))),
                all_bands_exact=bool(np.array_equal(aa,bb,equal_nan=True)))
        row['raster_bytes']={engine:path.stat().st_size for engine,path in paths.items()}
        # A complete one-survey offline archive fixture; no raw readers/publishers.
        index=out/'fused/v1/index.sqlite';index.parent.mkdir(parents=True,exist_ok=True)
        with sqlite3.connect(index) as db:
            db.executescript('DROP TABLE IF EXISTS rasters;'+Path('crates/lidar/src/schema.sql').read_text())
            db.execute('INSERT INTO rasters (url,survey,date,path,source_path,source_bytes,source_mtime_ns,'
                'source_sha256,raster_sha256,raster_bytes,source_crs,west,south,east,north,points,valid_points,'
                'valid_cells,zoom,created) VALUES ('+','.join('?'*20)+')',
                ('https://example.org/benchmark',fixture['survey'],'2026',paths['python'].name,'input.copc.laz',
                 fixture['bytes'],source.stat().st_mtime_ns,row['sha256'],sha(paths['python']),paths['python'].stat().st_size,
                 spec['source_crs'],*spec['bounds'],row['native_stats']['stats']['points'],
                 row['native_stats']['stats']['valid_points'],row['native_stats']['stats']['valid_cells'],15,0))
        catalog=out/'catalog.sqlite'
        with sqlite3.connect(catalog) as db:
            db.executescript('CREATE TABLE IF NOT EXISTS surveys (name TEXT,id TEXT PRIMARY KEY,page TEXT); DELETE FROM surveys;')
            db.execute('INSERT INTO surveys VALUES(?,?,?)',(fixture['name'],fixture['survey'],'https://example.org/benchmark'))
        box=maps.tile_bounds(fixture['z'],fixture['x'],fixture['y'])
        def native_read():
            run=subprocess.run([str(args.native),'render','--catalog',str(catalog),'--bounds',','.join(map(str,box)),
                                '--preferred',fixture['survey'],'--output',str(out/'native.png')],capture_output=True,text=True)
            if run.returncode:raise RuntimeError(run.stderr)
        for repeat in range(-1,args.runs):
            actions={'python':lambda:read_fused(paths['python'],box).save(out/'python.png'),'native':native_read}
            for engine in (['python','native'] if repeat%2 else ['native','python']):
                start=time.perf_counter();actions[engine]();elapsed=time.perf_counter()-start
                row['reads'].append(dict(repeat=repeat,engine=engine,seconds=elapsed))
        aa,bb=np.array(Image.open(out/'python.png')),np.array(Image.open(out/'native.png'))
        valid=aa[:,:,3]>0
        row['render_parity']=dict(alpha_mismatch=int(np.count_nonzero(aa[:,:,3]!=bb[:,:,3])),
            max_visible_rgb_error=int(np.abs(aa[:,:,:3].astype(int)-bb[:,:,:3].astype(int))[valid].max(initial=0)))
        for key in ['builds','reads']:
            row[key+'_medians']={engine:statistics.median(t['seconds'] for t in row[key]
                if t['engine']==engine and t['repeat']>=0) for engine in ['python','native']}
        report['fixtures'].append(row)
        (args.output/'results.json').write_text(json.dumps(report,indent=2)+'\n')
        print(json.dumps({key:row[key] for key in ['name','parity','render_parity','builds_medians','reads_medians']}),flush=True)
        if not row['parity']['all_bands_exact'] or row['render_parity']['alpha_mismatch']:
            raise RuntimeError('Native migration parity failed')


if __name__=='__main__':
    main()
