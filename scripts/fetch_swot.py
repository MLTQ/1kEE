#!/usr/bin/env python3
"""Prepare the Scripps SWOT_06 vertical gravity gradient for 1kEE (Python + GDAL)."""
import argparse
import datetime
import hashlib
import json
import math
import pathlib
import shutil
import subprocess
import tempfile

BASE = 'https://topex.ucsd.edu/pub/global_grav_1min_SWOT/'
SOURCE = BASE + 'curv_SWOT_06.nc'
README = BASE + 'README_SWOT_06.txt'
NATIVE = (21600, 10800)
OVERVIEW = (4096, 2048)


def run(*args):
    subprocess.run([str(a) for a in args], check=True)


def fetch(url, path):
    if path.exists():
        return
    part = path.with_suffix(path.suffix + '.part')
    # Keep interrupted transfers for curl's range resume, but publish only after
    # a successful complete response. No credentials are required.
    run('curl', '-fLsS', '--retry', '3', '--connect-timeout', '20', '--max-time',
        '1800', '-C', '-', url, '-o', part)
    part.replace(path)


def info(path):
    return json.loads(subprocess.check_output(['gdalinfo', '-json', str(path)]))


def validate_source(metadata):
    expected = [-180, 1 / 60, 0, 80, 0, -1 / 60]
    if metadata['size'] != [21600, 9600] or len(metadata['bands']) != 1:
        raise ValueError('Unexpected SWOT_06 dimensions/bands; refusing a different product')
    if not all(math.isclose(a, b, abs_tol=1e-8) for a, b in zip(metadata['geoTransform'], expected)):
        raise ValueError('Unexpected SWOT_06 registration/extent')
    band = metadata['bands'][0]
    if band['type'] != 'Float32' or band.get('scale', 1) != 1 or band.get('offset', 0) != 0:
        raise ValueError('Expected physical Float32 Eötvös values, without packed scaling')


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def check_grid(path, size):
    meta = info(path)
    if meta['size'] != list(size) or path.stat().st_size != size[0] * size[1] * 2:
        raise ValueError('Incomplete prepared grid: ' + str(path))
    if meta['bands'][0]['type'] != 'Int16' or meta['bands'][0]['noDataValue'] != -32768:
        raise ValueError('Invalid grid type/nodata')
    expected = [-180, 360 / size[0], 0, 90, 0, -180 / size[1]]
    if not all(math.isclose(a, b, abs_tol=1e-8) for a, b in zip(meta['geoTransform'], expected)):
        raise ValueError('Invalid prepared registration')
    if 'BYTEORDER      I' not in path.with_suffix('.hdr').read_text():
        raise ValueError('Prepared grids must be little endian')


def prepare(source, output, native=NATIVE, overview=OVERVIEW):
    """Publish a complete pair atomically; refuse replacement of existing data."""
    if output.exists():
        raise FileExistsError('Prepared directory exists; choose another --out-dir: ' + str(output))
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='.swot-build-', dir=output.parent) as scratch:
        scratch = pathlib.Path(scratch)
        scaled = scratch / 'scaled.tif'
        # Source is Float32 in Eötvös. Quantize at 0.1 E, preserving NaN as the
        # signed Int16 missing value. The explicit scale is exactly multiply 10.
        run('gdal_translate', '-q', '-of', 'GTiff', '-ot', 'Int16', '-scale',
            '-3276.8', '3276.7', '-32768', '32767', '-a_nodata', '-32768', source, scaled)
        staging = scratch / 'swot'
        staging.mkdir()
        full = staging / f'vgg_{native[0]}x{native[1]}.bil'
        run('gdalwarp', '-q', '-overwrite', '-of', 'EHdr', '-ot', 'Int16',
            '-s_srs', 'EPSG:4326', '-t_srs', 'EPSG:4326',
            '-te', '-180', '-90', '180', '90', '-ts', *native, '-r', 'near',
            '-srcnodata', '-32768', '-dstnodata', '-32768', scaled, full)
        small = staging / f'vgg_{overview[0]}x{overview[1]}.bil'
        run('gdal_translate', '-q', '-of', 'EHdr', '-ot', 'Int16',
            '-outsize', *overview, '-r', 'average', full, small)
        check_grid(full, native)
        check_grid(small, overview)
        manifest = {
            'schema': 1, 'product': 'SWOT_06 vertical gravity gradient',
            'release_date': '2026-09-15', 'retrieved_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
            'source_url': SOURCE, 'source_sha256': digest(source),
            'source_notes_url': README, 'units': 'Eotvos', 'scale': 0.1,
            'nodata': -32768, 'byte_order': 'little', 'bounds': [-180, -90, 180, 90],
            'source_latitude_coverage': [-80, 80],
            'citations': ['https://doi.org/10.1126/science.ads4472',
                          'https://doi.org/10.1016/j.asr.2019.09.011'],
            'grids': [{'file': p.name, 'size': list(size), 'sha256': digest(p)}
                      for p, size in [(full, native), (small, overview)]],
            'processing': 'Eotvos times 10 rounded to Int16; native nearest-neighbor global registration; overview area average; missing polar caps remain nodata; source land values preserved',
        }
        (staging / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
        notes = source.parent / 'README_SWOT_06.txt'
        if notes.exists():
            shutil.copy2(notes, staging / notes.name)
        staging.rename(output)
    return output


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--asset-root', required=True, type=pathlib.Path,
                        help='Existing data drive/root containing Data and Derived (e.g. /Volumes/Hilbert)')
    parser.add_argument('--source-dir', type=pathlib.Path)
    parser.add_argument('--out-dir', type=pathlib.Path)
    args = parser.parse_args()
    if not args.asset_root.is_dir():
        parser.error('Asset root is not available; mount the data drive first')
    args.source_dir = args.source_dir or args.asset_root / 'Data/SWOT/SWOT_06'
    args.out_dir = args.out_dir or args.asset_root / 'Derived/swot'
    for tool in ['curl', 'gdalinfo', 'gdal_translate', 'gdalwarp']:
        if not shutil.which(tool):
            parser.error('Required tool missing: ' + tool)
    if args.out_dir.exists():
        parser.error('Prepared directory already exists; choose another --out-dir')
    args.source_dir.mkdir(parents=True, exist_ok=True)
    source = args.source_dir / 'curv_SWOT_06.nc'
    fetch(SOURCE, source)
    fetch(README, args.source_dir / 'README_SWOT_06.txt')
    validate_source(info(source))
    print('Preparing native and overview gravity grids…', flush=True)
    prepare(source, args.out_dir)
    print(f'Prepared {args.out_dir.resolve()}. In 1kEE, enable Layers → SWOT marine gravity.')


if __name__ == '__main__':
    main()
