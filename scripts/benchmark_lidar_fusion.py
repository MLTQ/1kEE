#!/usr/bin/env python3
"""Compare full-scan raster preparation and current/fused map reads on real LAS/LAZ.

Run with the lidar-map Python dependencies plus rasterio. Inputs are read-only;
all generated files go in --output. A manifest lists name, path, survey, z, x, y.
"""
import argparse
import hashlib
import json
from importlib.metadata import version
import os
from pathlib import Path
import platform
import statistics
import subprocess
import time
from unittest.mock import patch

import laspy
import numpy as np
from pyproj import Transformer
import rasterio

import lidar_map as maps
from benchmarks.lidar_fusion_reference import grid, python_build


def pdal_pipeline(path, output, spec):
    return {'pipeline': [
        {'type':'readers.las', 'filename':str(path), 'override_srs':spec['crs']},
        {'type':'filters.expression', 'expression':'Classification != 7 && Classification != 18 && Withheld == 0'},
        {'type':'filters.reprojection', 'out_srs':'EPSG:4326'},
        {'type':'writers.gdal', 'filename':str(output), 'resolution':spec['resolution'],
         'origin_x':spec['w'], 'origin_y':spec['s'], 'width':spec['width'], 'height':spec['height'],
         'binmode':True, 'output_type':'min,max,mean,count', 'data_type':'float',
         'gdalopts':'COMPRESS=DEFLATE,PREDICTOR=3,TILED=YES'}]}


def timed(action):
    start = time.perf_counter()
    value = action()
    return time.perf_counter()-start, value


def read_fused(path, box):
    from rasterio.vrt import WarpedVRT
    from rasterio.transform import from_bounds
    with rasterio.open(path) as ds, WarpedVRT(ds, crs='EPSG:4326',
            transform=from_bounds(*box, maps.SIZE, maps.SIZE), width=maps.SIZE,
            height=maps.SIZE, nodata=float('nan')) as vrt:
        return maps.elevation_image(vrt.read(3, masked=True).filled(np.nan), box)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--manifest', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--gpupdal', type=Path, required=True)
    parser.add_argument('--runs', type=int, default=3)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    report = dict(machine=platform.uname()._asdict(), runs=args.runs,
                  protocol='One warmup, alternating full-scan builds; OS/drive caches uncontrolled. '
                  'Current COPC reads use resolution LOD; full-scan fused rasters use every valid point. '
                  'Times exclude interpreter startup, include child CLI startup and raster compression.', fixtures=[])
    report['dependencies']={name:version(name) for name in ['numpy','pillow','laspy','lazrs','pyproj','rasterio']}
    report['dependencies'].update(python=platform.python_version(),gdal=rasterio.__gdal_version__)
    report['cpu_model']=next((line.split(':',1)[1].strip() for line in Path('/proc/cpuinfo').read_text().splitlines()
                              if line.startswith('model name')),'unknown') if Path('/proc/cpuinfo').exists() else platform.processor()
    report['logical_cpus']=os.cpu_count()
    report['gpu'] = subprocess.run(['nvidia-smi','--query-gpu=name,driver_version','--format=csv,noheader'],
                                   capture_output=True, text=True).stdout
    report['doctor'] = subprocess.run([str(args.gpupdal),'doctor'], capture_output=True,text=True).stdout
    for fixture in json.loads(args.manifest.read_text()):
        path = Path(fixture['path'])
        out = args.output / fixture['name']
        out.mkdir(exist_ok=True)
        spec = grid(path)
        spec['source_crs'] = spec.get('source_crs', spec['crs'])
        if spec['width']*spec['height'] > 16_000_000:
            raise ValueError('Fixture too large for this bounded benchmark')
        with path.open('rb') as source:
            source_sha=hashlib.file_digest(source,'sha256').hexdigest()
        result = dict(fixture, sha256=source_sha,
                      grid={k:v for k,v in spec.items() if k not in ('crs','source_crs')}, builds=[], reads=[])
        paths = {engine:out/(engine+'.tif') for engine in ['python','gpupdal']}
        pipeline = out/'pipeline.json'
        pipeline.write_text(json.dumps(pdal_pipeline(path,paths['gpupdal'],spec),indent=2))
        def gpupdal():
            run = subprocess.run([str(args.gpupdal),'pipeline',str(pipeline)],capture_output=True,text=True,timeout=600)
            (out/'gpupdal.log').write_text(run.stdout+run.stderr)
            run.check_returncode()
        actions = {'python':lambda:python_build(path,paths['python'],spec), 'gpupdal':gpupdal}
        for repeat in range(-1,args.runs):
            for engine in (['python','gpupdal'] if repeat%2 else ['gpupdal','python']):
                seconds,_ = timed(actions[engine])
                result['builds'].append(dict(repeat=repeat,engine=engine,seconds=seconds))
                print(f"{fixture['name']} build {engine} {repeat}: {seconds:.3f}s",flush=True)
        with rasterio.open(paths['python']) as a, rasterio.open(paths['gpupdal']) as b:
            aa, bb = a.read(masked=True).filled(np.nan), b.read(masked=True).filled(np.nan)
            bb[3][bb[3]==0] = np.nan
            valid = np.isfinite(aa[2]) & np.isfinite(bb[2])
            result['parity'] = dict(valid_cells=int(valid.sum()),
                coverage_mismatch=int(np.count_nonzero(np.isfinite(aa[2])!=np.isfinite(bb[2]))),
                count_mismatch=int(np.count_nonzero(np.nan_to_num(aa[3])!=np.nan_to_num(bb[3]))),
                max_mean_error_m=float(np.max(np.abs(aa[2][valid]-bb[2][valid]))) if valid.any() else None,
                p95_mean_error_m=float(np.percentile(np.abs(aa[2][valid]-bb[2][valid]),95)) if valid.any() else None,
                valid_points=int(np.nansum(aa[3])),
                max_extrema_error_m=float(np.nanmax(np.abs(aa[:2]-bb[:2]))))
        result['raster_bytes']={k:p.stat().st_size for k,p in paths.items()}
        box = maps.tile_bounds(fixture['z'],fixture['x'],fixture['y'])
        for repeat in range(-1,args.runs):
            actions = {'current':lambda:maps.local_cloud([('copc',path,None)],box),
                       'fused':lambda:read_fused(paths['python'],box)}
            for engine in (['current','fused'] if repeat%2 else ['fused','current']):
                seconds,image = timed(actions[engine])
                result['reads'].append(dict(repeat=repeat,engine=engine,seconds=seconds))
                image.save(out/(engine+'.png'))
        # Record the visible change caused by full density, not only numeric encoding.
        with patch.object(maps,'elevation_image',side_effect=lambda values,box:values):
            current=maps.local_cloud([('copc',path,None)],box)
            fused=read_fused(paths['python'],box)
        both=np.isfinite(current)&np.isfinite(fused)
        error=np.abs(current[both]-fused[both])
        result['current_lod_comparison']=dict(current_pixels=int(np.isfinite(current).sum()),
            fused_pixels=int(np.isfinite(fused).sum()),shared_pixels=int(both.sum()),
            p50_height_difference_m=float(np.median(error)) if error.size else None,
            p95_height_difference_m=float(np.percentile(error,95)) if error.size else None,
            max_height_difference_m=float(error.max()) if error.size else None)
        for key,engines in [('builds',['python','gpupdal']),('reads',['current','fused'])]:
            result[key+'_medians']={engine:statistics.median(r['seconds'] for r in result[key]
                if r['engine']==engine and r['repeat']>=0) for engine in engines}
        report['fixtures'].append(result)
        (args.output/'results.json').write_text(json.dumps(report,indent=2))
        print(json.dumps({k:result[k] for k in ['name','parity','builds_medians','reads_medians','current_lod_comparison']}),flush=True)


if __name__=='__main__':
    main()
