#!/usr/bin/env python3
"""Linux cold-source benchmark. Reads sources/schema only; writes fresh /tmp caches.

Requests per-source POSIX_FADV_DONTNEED before each fresh process. This is best
 effort, not a global cache flush or a guarantee of cold drive-controller caches.
Reported input bytes are process/descendant ru_inblock*512, not source-only I/O.
"""
import argparse
import json
import math
import os
from pathlib import Path
import resource
import sqlite3
import struct
import subprocess
import tempfile
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--srtm-root', type=Path, required=True)
    parser.add_argument('--schema-db', type=Path, required=True,
                        help='Existing valid native cache: copies schema, never geometry')
    parser.add_argument('--label', default='cold')
    parser.add_argument('--gpu', default='RTX 4090')
    args = parser.parse_args()
    binary, source = args.binary.resolve(strict=True), args.srtm_root.resolve(strict=True)
    root = Path(tempfile.mkdtemp(prefix='1kee-cold-', dir='/tmp'))
    seed = sqlite3.connect(args.schema_db.resolve(strict=True).as_uri() + '?mode=ro&immutable=1', uri=True)
    empty = sqlite3.connect(':memory:')
    tables = ['contour_tiles', 'contour_tile_manifest', 'coastline_tiles',
              'coastline_tile_manifest', 'contour_source_metadata']
    for table in tables:
        row = seed.execute("SELECT sql FROM sqlite_master WHERE type='table' AND name=?", (table,)).fetchone()
        if row is None:
            raise RuntimeError(f'Missing native schema table: {table}')
        empty.execute(row[0])
    for (sql,) in seed.execute("SELECT sql FROM sqlite_master WHERE type='index' AND sql IS NOT NULL AND tbl_name IN ('contour_tiles','coastline_tiles')"):
        empty.execute(sql)
    quality = seed.execute("SELECT value FROM contour_source_metadata WHERE key='quality'").fetchone()
    if quality != ('srtm-gl1-native-v1',):
        raise RuntimeError('Schema source must be a native SRTM cache')
    empty.execute("INSERT INTO contour_source_metadata VALUES('quality',?)", quality)
    empty.commit()
    seed.close()
    f32 = lambda v: struct.unpack('f', struct.pack('f', v))[0]
    for z, y, x, half in [(4, 126, 143, .55), (0, 9, 27, 3.6), (4, -98, -235, .55)]:
        # Conservative source-file bounds around CoreTile's native aligned halo.
        step = f32(f32(half) * f32(.45))
        paths = []
        for lat in range(math.floor((y-.5)*step-3/3600), math.floor((y+.5)*step+3/3600)+1):
            for lon in range(math.floor((x-.5)*step-3/3600), math.floor((x+.5)*step+3/3600)+1):
                path = source / f'{"N" if lat>=0 else "S"}{abs(lat):02d}{"E" if lon>=0 else "W"}{abs(lon):03d}.tif'
                if path.exists():
                    paths.append(path)
        if not paths:
            raise RuntimeError(f'No sources for fixture {z}/{y}/{x}')
        for repeat in range(2):
            for gpu in ([False, True] if repeat == 0 else [True, False]):
                dest = root / f'{z}_{y}_{x}_{repeat}_{gpu}' / 'srtm_native_v1.sqlite'
                dest.parent.mkdir()
                connection = sqlite3.connect(dest)
                empty.backup(connection)
                connection.close()
                for path in paths:
                    fd = os.open(path, os.O_RDONLY)
                    try:
                        os.posix_fadvise(fd, 0, 0, os.POSIX_FADV_DONTNEED)
                    finally:
                        os.close(fd)
                env = os.environ.copy()
                env.update(ONEKEE_GPU=args.gpu, ONEKEE_TERRAIN_GPU=str(int(gpu)),
                           ONEKEE_TERRAIN_REPAIR_ROOT=str(root), ONEKEE_TERRAIN_TIMINGS='1', GDAL_PAM_ENABLED='NO')
                before = resource.getrusage(resource.RUSAGE_CHILDREN).ru_inblock
                start = time.monotonic()
                process = subprocess.run([str(binary), '--repair-native-tile', str(dest), str(source), str(z), str(y), str(x)],
                                         env=env, capture_output=True, text=True, timeout=90)
                elapsed = time.monotonic()-start
                log = dest.parent / 'build.log'
                log.write_text(process.stdout+process.stderr)
                print(json.dumps(dict(label=args.label, tile=[z,y,x], round=repeat, gpu=gpu, seconds=elapsed,
                      input_bytes=(resource.getrusage(resource.RUSAGE_CHILDREN).ru_inblock-before)*512,
                      returncode=process.returncode, log=str(log))), flush=True)
                if process.returncode or (gpu and 'Falling back' in process.stderr):
                    raise RuntimeError(process.stderr)
    empty.close()


if __name__ == '__main__':
    main()
