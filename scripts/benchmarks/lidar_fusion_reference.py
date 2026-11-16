#!/usr/bin/env python3
"""Frozen Python comparison oracle for the native Rust LiDAR pipeline.

Benchmark use only; production preparation and fusion live in crates/lidar.
"""
import math
import laspy
import numpy as np
from pyproj import Transformer
import rasterio
from rasterio.transform import from_origin



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
