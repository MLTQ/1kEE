#!/usr/bin/env python3
"""Prepare resumable numeric LiDAR rasters; retain publisher and acquisition receipts.

The map fuses these rasters newest-first at measured pixels. Raw scans are never
deleted by this command. Use --plan to inspect the work before building.
"""
import argparse
from contextlib import closing
import fcntl
import hashlib
import json
import math
import os
from pathlib import Path
import shutil
import sqlite3
import tempfile
import time

import laspy
import numpy as np
from pyproj import Transformer
import rasterio
from rasterio.transform import from_origin

SCHEMA = """
CREATE TABLE IF NOT EXISTS rasters (
 id INTEGER PRIMARY KEY AUTOINCREMENT, url TEXT UNIQUE NOT NULL,
 survey TEXT NOT NULL, date TEXT NOT NULL, path TEXT NOT NULL,
 source_path TEXT NOT NULL, source_bytes INTEGER NOT NULL, source_mtime_ns INTEGER NOT NULL,
 source_sha256 TEXT NOT NULL, raster_sha256 TEXT NOT NULL, raster_bytes INTEGER NOT NULL,
 source_crs TEXT NOT NULL, west REAL, south REAL, east REAL, north REAL,
 points INTEGER NOT NULL, valid_points INTEGER NOT NULL, valid_cells INTEGER NOT NULL,
 zoom INTEGER NOT NULL, created REAL NOT NULL
);
CREATE INDEX IF NOT EXISTS raster_location ON rasters(west,east);
CREATE INDEX IF NOT EXISTS raster_survey ON rasters(survey);
CREATE TABLE IF NOT EXISTS failures (url TEXT PRIMARY KEY, error TEXT NOT NULL, updated REAL NOT NULL);
"""


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def grid(path, zoom=15):
    with laspy.open(path) as reader:
        if reader.header.point_count == 0:
            raise ValueError('Empty point cloud')
        original = reader.header.parse_crs()
        if original is None:
            raise ValueError('Missing point-cloud coordinate system')
        crs = original.to_2d()
        bounds = Transformer.from_crs(crs, 4326, always_xy=True).transform_bounds(
            *reader.header.mins[:2], *reader.header.maxs[:2])
    if not all(math.isfinite(v) for v in bounds):
        raise ValueError('Invalid geographic bounds')
    resolution = 180 / 2**zoom / 512
    w, s, e, n = bounds
    if not (-180 <= w <= e <= 180 and -90 <= s <= n <= 90):
        raise ValueError('Unsupported wrapped or invalid geographic bounds')
    # Global app pixel grid plus a one-pixel halo; no interpolation into gaps.
    left, bottom = math.floor((w+180)/resolution)-1, math.floor((s+90)/resolution)-1
    right, top = math.ceil((e+180)/resolution)+1, math.ceil((n+90)/resolution)+1
    return dict(w=left*resolution-180, s=bottom*resolution-90, width=right-left,
                height=top-bottom, resolution=resolution, crs=crs.to_wkt(),
                source_crs=original.to_wkt(), bounds=bounds)


def python_build(path, output, spec):
    """All valid returns, double precision accumulation; float32 numeric storage.

    Min/max are observed returns, not a classified DTM/DSM. Counts and mean
    preserve the display's all-return averaging contract at this resolution.
    """
    h, w, res = spec['height'], spec['width'], spec['resolution']
    if w*h > 16_000_000:
        raise ValueError('Source exceeds bounded 16-million-cell raster; needs tiled ingestion')
    sums, counts = np.zeros(w*h), np.zeros(w*h, dtype='uint32')
    low, high = np.full(w*h, np.inf), np.full(w*h, -np.inf)
    transform = Transformer.from_crs(spec['crs'], 4326, always_xy=True)
    seen, accepted = 0, 0
    with laspy.open(path) as reader:
        for p in reader.chunk_iterator(250_000):
            seen += len(p)
            keep = ~np.isin(p.classification, [7, 18]) & (np.asarray(p.withheld) == 0)
            lon, lat = transform.transform(np.asarray(p.x)[keep], np.asarray(p.y)[keep])
            z = np.asarray(p.z)[keep]
            if not np.isfinite(lon).all() or not np.isfinite(lat).all() or not np.isfinite(z).all():
                raise ValueError('Nonfinite transformed measurement')
            col = np.floor((lon-spec['w'])/res).astype(int)
            row = h-1-np.floor((lat-spec['s'])/res).astype(int)
            valid = (col>=0) & (col<w) & (row>=0) & (row<h)
            if not valid.all():
                raise ValueError('Transformed points outside header-derived raster')
            index = row*w+col
            accepted += len(index)
            sums += np.bincount(index, weights=z, minlength=w*h)
            increment = np.bincount(index, minlength=w*h)
            if np.any(increment > np.iinfo('uint32').max-counts):
                raise ValueError('Point-count overflow')
            counts += increment.astype('uint32')
            np.minimum.at(low, index, z)
            np.maximum.at(high, index, z)
        if seen != reader.header.point_count:
            raise ValueError('Incomplete point-cloud decode')
    if counts.max(initial=0) > 2**24:
        raise ValueError('Counts exceed exact float32 storage')
    mean = np.divide(sums, counts, out=np.full(w*h, np.nan), where=counts>0)
    low[counts==0], high[counts==0] = np.nan, np.nan
    bands = np.stack([low, high, mean, np.where(counts>0, counts, np.nan)]).reshape(4,h,w).astype('float32')
    with rasterio.open(output, 'w', driver='GTiff', width=w, height=h, count=4,
                       dtype='float32', crs='EPSG:4326', nodata=float('nan'),
                       transform=from_origin(spec['w'], spec['s']+h*res, res, res),
                       compress='deflate', predictor=3, tiled=True) as ds:
        ds.write(bands)
        ds.descriptions = ('min', 'max', 'mean', 'count')
        ds.update_tags(source_crs=spec['source_crs'], product='all-return elevation statistics')
    # Decode the stored product and verify masks, every statistic, and point count.
    with rasterio.open(output) as ds:
        if not np.array_equal(ds.read(), bands, equal_nan=True):
            raise ValueError('Numeric raster round-trip mismatch')
    if int(counts.astype('uint64').sum()) != accepted:
        raise ValueError('Stored point-count mismatch')
    return dict(points=seen, valid_points=accepted, valid_cells=int(np.count_nonzero(counts)))


def assets(root, catalog):
    """Only completed, present, directly readable scan receipts; newest first."""
    with closing(sqlite3.connect((root/'bulk.sqlite').resolve().as_uri()+'?mode=ro', uri=True)) as bulk:
        rows = list(bulk.execute("SELECT url,survey,path,bytes FROM files WHERE kind='copc'"))
    dates = dict(catalog.execute('SELECT id,coalesce(date,\'\') FROM surveys'))
    for url, survey, path, size in sorted(rows, key=lambda r:(dates.get(r[1],''),r[1],r[0]), reverse=True):
        source = root/path
        if not source.resolve().is_relative_to(root.resolve()):
            raise ValueError('Receipt source escapes lidar root')
        if source.is_file():
            if source.stat().st_size != size:
                continue  # A partial/replaced file is not a completed receipt.
            yield dict(url=url, survey=survey, path=source, date=dates.get(survey,''))


def prepare(root, db, asset, zoom=15):
    source = asset['path']
    before = source.stat()
    old = db.execute('SELECT source_bytes,source_mtime_ns,zoom,path,raster_sha256,date FROM rasters WHERE url=?',
                     [asset['url']]).fetchone()
    if old and tuple(old[:3]) == (before.st_size,before.st_mtime_ns,zoom) and old[5] == asset['date']:
        output = root/old[3]
        if output.is_file() and digest(output)==old[4]:
            return 'reused'
    spec = grid(source, zoom)
    if spec['width']*spec['height'] > 16_000_000:
        raise ValueError('Source exceeds bounded 16-million-cell raster; needs tiled ingestion')
    # Acquisition keeps 100 GB for downloads. Bounded conversion uses that
    # headroom, retaining 10 GB plus space for two uncompressed raster copies.
    required = 10_000_000_000 + spec['width']*spec['height']*32
    if shutil.disk_usage(root).free < required:
        raise OSError('Paused: fused build requires 10 GB reserve plus raster scratch')
    sha = digest(source)
    folder = root/'fused'/'v1'/'rasters'
    folder.mkdir(parents=True, exist_ok=True)
    # Content versioned files keep old readers and the old manifest valid until commit.
    name = hashlib.sha256((asset['url']+sha+str(zoom)).encode()).hexdigest()+'.tif'
    output = folder/name
    with tempfile.NamedTemporaryFile(dir=folder, suffix='.tif', delete=False) as tmp:
        temporary = Path(tmp.name)
    try:
        stats = python_build(source, temporary, spec)
        with rasterio.open(temporary, 'r+') as ds:
            ds.update_tags(survey=asset['survey'], acquisition=asset['date'],
                           publisher=asset['url'], source_sha256=sha)
        after = source.stat()
        if (before.st_size,before.st_mtime_ns) != (after.st_size,after.st_mtime_ns):
            raise ValueError('Source changed during conversion')
        raster_sha, raster_bytes = digest(temporary), temporary.stat().st_size
        with temporary.open('rb') as stream:
            os.fsync(stream.fileno())
        os.replace(temporary, output)
        values = (asset['url'],asset['survey'],asset['date'],str(output.relative_to(root)),
                  str(source.relative_to(root)),before.st_size,before.st_mtime_ns,sha,raster_sha,raster_bytes,
                  spec['source_crs'],*spec['bounds'],stats['points'],stats['valid_points'],
                  stats['valid_cells'],zoom,time.time())
        db.execute('INSERT OR REPLACE INTO rasters (url,survey,date,path,source_path,source_bytes,'
                   'source_mtime_ns,source_sha256,raster_sha256,raster_bytes,source_crs,west,south,east,north,'
                   'points,valid_points,valid_cells,zoom,created) VALUES ('+','.join('?'*20)+')',values)
        db.execute('DELETE FROM failures WHERE url=?',[asset['url']])
        db.commit()
        return 'built'
    finally:
        temporary.unlink(missing_ok=True)


def verify(root):
    """Audit derived bytes and point counts before any separate retention action."""
    failures=[]
    total=source_bytes=raster_bytes=0
    with closing(sqlite3.connect((root/'fused/v1/index.sqlite').resolve().as_uri()+'?mode=ro',uri=True)) as db:
        rows=db.execute('SELECT path,raster_sha256,source_bytes,raster_bytes,valid_points FROM rasters').fetchall()
    for path,sha,raw_size,size,points in rows:
        try:
            file=root/path
            if digest(file)!=sha or file.stat().st_size!=size:
                raise ValueError('Raster checksum/size mismatch')
            with rasterio.open(file) as ds:
                data=ds.read()
                if ds.descriptions!=('min','max','mean','count'):
                    raise ValueError('Raster bands mismatch')
            valid=np.isfinite(data[2])
            if (not all(np.array_equal(np.isfinite(band),valid) for band in data)
                    or np.any(data[0][valid]>data[2][valid]) or np.any(data[2][valid]>data[1][valid])
                    or np.any(data[3][valid]<1) or np.any(data[3][valid]%1)
                    or int(np.nansum(data[3].astype('float64')))!=points):
                raise ValueError('Raster statistic/count mismatch')
            total+=1
            source_bytes+=raw_size
            raster_bytes+=size
        except (OSError,ValueError,rasterio.errors.RasterioError) as error:
            failures.append(dict(path=path,error=str(error)))
    return dict(verified=total,source_bytes=source_bytes,raster_bytes=raster_bytes,failures=failures)


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root',type=Path,required=True,help='Existing Data/Lidar directory')
    parser.add_argument('--plan',action='store_true')
    parser.add_argument('--verify',action='store_true',help='Audit all published raster checksums and numeric statistics')
    parser.add_argument('--max-files',type=int)
    parser.add_argument('--volume-uuid',help='Require this filesystem UUID before conversion')
    parser.add_argument('--zoom',type=int,default=15,choices=range(12,17))
    args=parser.parse_args()
    if args.max_files is not None and args.max_files < 1:
        parser.error('--max-files must be positive')
    root=args.root.resolve(strict=True)
    import lidar_catalog as catalog_module
    catalog_module.check_removable_mount(root/'catalog.sqlite')
    # Keep relative I/O pinned to the opened data volume, even after lazy unmount.
    os.chdir(root)
    if args.volume_uuid:
        import subprocess
        identity=subprocess.check_output(['findmnt','--noheadings','--raw','--output','MAJ:MIN,UUID',
                                          '--target','.'],text=True).split()
        device=os.stat('.').st_dev
        if (len(identity)!=2 or identity[0]!=f'{os.major(device)}:{os.minor(device)}'
                or identity[1].casefold()!=args.volume_uuid.casefold()):
            parser.error('LiDAR data volume identity mismatch')
    root=Path('.')
    if args.verify:
        report=verify(root)
        print(json.dumps(report))
        return int(bool(report['failures']))
    with closing(sqlite3.connect((root/'catalog.sqlite').resolve().as_uri()+'?mode=ro',uri=True)) as catalog:
        planned=list(assets(root,catalog))
    if args.max_files is not None:
        planned=planned[:args.max_files]
    if args.plan:
        print(json.dumps(dict(files=len(planned),source_bytes=sum(a['path'].stat().st_size for a in planned),
                              newest_first=True,zoom=args.zoom,raw_deletion=False)))
        return
    folder=root/'fused'/'v1'
    folder.mkdir(parents=True,exist_ok=True)
    with (folder/'build.lock').open('a') as lock:
        fcntl.flock(lock,fcntl.LOCK_EX|fcntl.LOCK_NB)
        with closing(sqlite3.connect(folder/'index.sqlite')) as db:
            db.executescript(SCHEMA)
            for asset in planned:
                start=time.monotonic()
                try:
                    status=prepare(root,db,asset,args.zoom)
                except OSError:
                    raise  # Pause the batch on full/offline volume instead of churning failures.
                except (ValueError,RuntimeError) as error:
                    db.execute('INSERT OR REPLACE INTO failures VALUES(?,?,?)',
                               (asset['url'],str(error),time.time()))
                    db.commit()
                    status=str(error)
                print(json.dumps(dict(survey=asset['survey'],status=status,
                                      seconds=time.monotonic()-start)),flush=True)


if __name__=='__main__':
    raise SystemExit(main())
