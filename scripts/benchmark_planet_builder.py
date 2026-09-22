"""Compare two planet-all binaries using disposable output and exact cell data."""
import argparse
import hashlib
import json
from pathlib import Path
import struct
import subprocess
import tempfile
import time


def cell_signature(root):
    """Ignore HashMap serialization order, retaining all feature bytes and IDs."""
    digest = hashlib.sha256()
    files = features = points = size = 0
    for path in sorted(Path(root).rglob("*.1kc")):
        data = path.read_bytes()
        if data[:5] != b"1kEE\x01" or len(data) < 10:
            raise ValueError(f"Invalid cell header: {path}")
        digest.update(str(path.relative_to(root)).encode() + b"\0" + data[:10])
        offset = 10
        while offset < len(data):
            tag = data[offset:offset + 4]
            length, = struct.unpack_from("<I", data, offset + 4)
            end = offset + 8 + length
            if end > len(data):
                raise ValueError(f"Truncated chunk: {path}")
            count, = struct.unpack_from("<I", data, offset + 8)
            offset += 12
            records = []
            for _ in range(count):
                start = offset
                flags = data[offset + 9]
                name_length, = struct.unpack_from("<H", data, offset + 10)
                offset += 12 + name_length
                point_count, = struct.unpack_from("<I", data, offset)
                offset += 4 + point_count * (12 if flags & 4 else 8)
                if offset > end:
                    raise ValueError(f"Truncated feature: {path}")
                records.append(data[start:offset])
                points += point_count
            if offset != end:
                raise ValueError(f"Unexpected chunk tail: {path}")
            digest.update(tag + struct.pack("<I", count))
            for record in sorted(records):
                digest.update(struct.pack("<Q", len(record)) + record)
            features += count
        files += 1
        size += len(data)
    return dict(files=files, features=features, points=points, bytes=size,
                sha256=digest.hexdigest())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--before", type=Path, required=True)
    parser.add_argument("--after", type=Path, required=True)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--rounds", type=int, choices=range(1, 11), default=3)
    args = parser.parse_args()
    reference = None
    for round_number in range(args.rounds):
        for label in ("before", "after"):
            with tempfile.TemporaryDirectory(prefix="1kee-planet-compare-") as tmp:
                root = Path(tmp)
                start = time.perf_counter()
                subprocess.run([
                    str(getattr(args, label).resolve()), "planet-all",
                    "--planet", str(args.source.resolve()),
                    "--out-dir", str(root / "out"), "--tmp-dir", str(root / "nodes"),
                    "--features", "all",
                ], check=True, stdout=subprocess.DEVNULL)
                elapsed = time.perf_counter() - start
                signature = cell_signature(root / "out")
                if reference is None:
                    reference = signature
                if signature != reference:
                    raise ValueError("Builder outputs differ; speed result is not valid")
                print(json.dumps(dict(round=round_number + 1, build=label,
                                      seconds=elapsed, **signature)), flush=True)


if __name__ == "__main__":
    main()
