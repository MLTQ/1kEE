#!/usr/bin/env bash
# Opt-in GPU SRTM generation plus the existing RTX 4090 rendering profile.
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."
export ONEKEE_TERRAIN_GPU="${ONEKEE_TERRAIN_GPU:-1}"
exec ./scripts/run-4090.sh "$@"
