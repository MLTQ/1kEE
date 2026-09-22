"""Compare flat/indexed-PBF build time, scratch space and exact feature parity."""
import argparse
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import time

from benchmark_planet_builder import cell_signature


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--builder", type=Path, required=True)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--rounds", type=int, choices=range(1, 11), default=3)
    args = parser.parse_args()
    reference = None
    for round_number in range(args.rounds):
        modes = ["flat", "indexed-pbf"]
        if round_number % 2:
            modes.reverse()
        for mode in modes:
            with tempfile.TemporaryDirectory(prefix="1kee-node-compare-") as tmp:
                root = Path(tmp)
                start = time.perf_counter()
                command = [
                    str(args.builder.resolve()), "planet-all",
                    "--planet", str(args.source.resolve()),
                    "--out-dir", str(root / "out"), "--tmp-dir", str(root / "nodes"),
                    "--features", "all", "--node-storage", mode,
                ]
                if sys.platform == "darwin":
                    command = ["/usr/bin/time", "-l", *command]
                run = subprocess.run(command, capture_output=True, text=True)
                elapsed = time.perf_counter() - start
                if run.returncode:
                    raise RuntimeError(run.stderr)
                signature = cell_signature(root / "out")
                if reference is None:
                    reference = signature
                if signature != reference:
                    raise ValueError("Node storage modes produced different features")
                scratch = sum(p.stat().st_size for p in (root / "nodes").rglob("*") if p.is_file())
                rss = re.search(r"(\d+)\s+maximum resident set size", run.stderr)
                print(json.dumps(dict(source=args.source.name, round=round_number + 1,
                                      storage=mode, seconds=elapsed,
                                      scratch_bytes=scratch,
                                      peak_rss_bytes=int(rss[1]) if rss else None,
                                      **signature)), flush=True)


if __name__ == "__main__":
    main()
