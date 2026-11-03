#!/usr/bin/env python3
"""Prepare selected NCEI free-air, Bouguer and isostatic maps on a data drive."""
import argparse
import array
import datetime
import hashlib
import json
import math
import pathlib
import shutil
import subprocess
import sys
import tempfile
from xml.sax.saxutils import escape

CATALOG = pathlib.Path(__file__).resolve().parents[1] / 'crates/desktop/src/model/gravity_maps.json'
NODATA = -999999.0


def run(*args):
    subprocess.run([str(a) for a in args], check=True)


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def fetch(url, path):
    if path.exists():
        return
    part = path.with_suffix(path.suffix + '.part')
    run('curl', '-fLsS', '--retry', '3', '--connect-timeout', '20',
        '--max-time', '1800', '-C', '-', url, '-o', part)
    part.replace(path)


def physical_grid(source, spec, output):
    """Decode scaled legacy samples, reverse southern-first rows, preserve holes."""
    w, h = spec['size']
    item_size = {'h': 2, 'i': 4, 'f': 4}[spec['type']]
    if source.stat().st_size != w * h * item_size:
        raise ValueError('Unexpected source length: ' + str(source))
    missing = spec['missing']
    lo, hi, valid, absent = math.inf, -math.inf, 0, 0
    with source.open('rb') as src, output.open('wb') as dst:
        for y in range(h):
            row_index = h - y - 1 if spec['row_order'] == 'south_to_north' else y
            src.seek(row_index * w * item_size)
            row = array.array(spec['type'])
            row.frombytes(src.read(w * item_size))
            if sys.byteorder != 'little':
                row.byteswap()
            result = array.array('f')
            for packed in row:
                value = packed * spec['scale']
                if not math.isfinite(value) or (missing is not None and math.isclose(value, missing, abs_tol=.01, rel_tol=0)):
                    result.append(NODATA)
                    absent += 1
                else:
                    if not -2000 <= value <= 2000:
                        raise ValueError(f'Unrecognized value {value} in {spec["id"]}')
                    lo, hi = min(lo, value), max(hi, value)
                    result.append(value)
                    valid += 1
            if sys.byteorder != 'little':
                result.byteswap()
            dst.write(result.tobytes())
    if valid == 0:
        raise ValueError('Source contains no valid samples')
    return {'min_mGal': lo, 'max_mGal': hi, 'valid': valid, 'missing': absent}


def write_vrt(path, binary, spec):
    w, h = spec['size']
    transform = ', '.join(str(v) for v in spec['geotransform'])
    path.write_text(f'''<VRTDataset rasterXSize="{w}" rasterYSize="{h}">
<SRS>{escape(spec['srs'])}</SRS><GeoTransform>{transform}</GeoTransform>
<VRTRasterBand dataType="Float32" band="1" subClass="VRTRawRasterBand">
<NoDataValue>{NODATA}</NoDataValue><SourceFilename relativeToVRT="1">{escape(binary.name)}</SourceFilename>
<ImageOffset>0</ImageOffset><PixelOffset>4</PixelOffset><LineOffset>{w * 4}</LineOffset><ByteOrder>LSB</ByteOrder>
</VRTRasterBand></VRTDataset>''')


def prepare(source, spec, output):
    """Publish one complete regional product atomically; never replace data."""
    if output.exists():
        raise FileExistsError(str(output))
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='.ncei-build-', dir=output.parent) as temp:
        scratch = pathlib.Path(temp)
        physical = scratch / 'physical.f32'
        stats = physical_grid(source, spec, physical)
        vrt = scratch / 'physical.vrt'
        write_vrt(vrt, physical, spec)
        staging = scratch / spec['id']
        staging.mkdir()
        raster = staging / 'grid.bil'
        if spec['target_spacing'] is None:
            # Preserve the NGS geographic nodes exactly, including unwrapped Alaska.
            run('gdal_translate', '-q', '-of', 'EHdr', '-ot', 'Float32', vrt, raster)
        else:
            run('gdalwarp', '-q', '-of', 'EHdr', '-ot', 'Float32', '-t_srs', 'EPSG:4326',
                '-tr', spec['target_spacing'], spec['target_spacing'], '-r', 'near',
                '-et', '0', '-srcnodata', NODATA, '-dstnodata', NODATA, vrt, raster)
        meta = json.loads(subprocess.check_output(['gdalinfo', '-json', str(raster)]))
        w, h = meta['size']
        gt = meta['geoTransform']
        band = meta['bands'][0]
        if w <= 0 or h <= 0 or max(w, h) > 8192 or raster.stat().st_size != w * h * 4:
            raise ValueError('Unexpected prepared dimensions/length')
        if band['type'] != 'Float32' or band['noDataValue'] != NODATA or gt[2] != 0 or gt[4] != 0 or gt[1] <= 0 or gt[5] >= 0:
            raise ValueError('Unexpected prepared format/registration')
        if 'BYTEORDER      I' not in raster.with_suffix('.hdr').read_text():
            raise ValueError('Prepared grid must be little-endian')
        manifest = dict(schema=1, id=spec['id'], kind=spec['kind'], units='mGal',
                        size=[w, h], bounds=[gt[0], gt[3] + h * gt[5], gt[0] + w * gt[1], gt[3]],
                        nodata=NODATA, byte_order='little', source_url=spec['url'],
                        source_sha256=digest(source), grid_sha256=digest(raster),
                        source_notes_url=spec['notes_url'], source_spec=spec, source_statistics=stats,
                        prepared_utc=datetime.datetime.now(datetime.timezone.utc).isoformat(),
                        processing='Physical Float32 mGal; north-up rows; missing values preserved; geographic originals retained or nearest-neighbor reprojection sampled finer than source spacing; no land mask or cross-product mosaicking')
        (staging / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
        (staging / 'README.txt').write_text(f"{spec['label']} — {spec['kind']}\n{spec['note']}\nSource grid spacing: {spec['spacing']} (not guaranteed feature resolution).\n{spec['crs_note']}\nSource/attribution: {spec['notes_url']}\n")
        staging.rename(output)
    return manifest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--asset-root', type=pathlib.Path, required=True)
    parser.add_argument('--only', nargs='+', help='Optional subset of product IDs')
    args = parser.parse_args()
    if not args.asset_root.is_dir():
        parser.error('Mount the data drive first; asset root must exist')
    for tool in ['curl', 'gdalinfo', 'gdal_translate', 'gdalwarp']:
        if not shutil.which(tool):
            parser.error('Required tool missing: ' + tool)
    specs = json.loads(CATALOG.read_text())
    if args.only and set(args.only) - {s['id'] for s in specs}:
        parser.error('Unknown product ID')
    for spec in specs:
        if args.only and spec['id'] not in args.only:
            continue
        if not spec['id'].replace('_', '').isalnum():
            raise ValueError('Unsafe product ID')
        folder = args.asset_root / 'Data/Gravity/NCEI/maps' / spec['id']
        folder.mkdir(parents=True, exist_ok=True)
        source = folder / (spec['id'] + '.bin')
        fetch(spec['url'], source)
        if source.stat().st_size != spec['bytes'] or digest(source) != spec['sha256']:
            raise ValueError('Source changed or is incomplete; refusing guessed layout: ' + str(source))
        for ext in ['hdr', 'fmt']:
            fetch(spec['url'].rsplit('.', 1)[0] + '.' + ext, folder / (spec['id'] + '.' + ext))
        fetch(spec['notes_url'], folder / 'source-notes.txt')
        output = args.asset_root / 'Derived/gravity/ncei' / spec['id']
        if output.exists():
            manifest = json.loads((output / 'manifest.json').read_text())
            if manifest.get('source_spec') != spec or digest(output / 'grid.bil') != manifest['grid_sha256']:
                raise ValueError('Existing output differs; choose a separate root or move it aside explicitly')
            print(f'{spec["id"]}: existing output verified', flush=True)
            continue
        manifest = prepare(source, spec, output)
        print(f'{spec["id"]}: prepared {manifest["size"]} on {args.asset_root}', flush=True)
    print('In 1kEE: Layers → Gravity anomaly → quantity and region.', flush=True)


if __name__ == '__main__':
    main()
