#!/usr/bin/env bash
# Opt-in profile; explicit environment settings take precedence.
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."
export ONEKEE_GPU="${ONEKEE_GPU:-RTX 4090}"
export ONEKEE_GPU_CACHE_MIB="${ONEKEE_GPU_CACHE_MIB:-8192}"
export ONEKEE_CPU_CACHE_MIB="${ONEKEE_CPU_CACHE_MIB:-12288}"
export ONEKEE_ATLAS_RAM_MIB="${ONEKEE_ATLAS_RAM_MIB:-8192}"
export ONEKEE_ATLAS_DISK_MIB="${ONEKEE_ATLAS_DISK_MIB:-32768}"
export ONEKEE_UPLOAD_MIB="${ONEKEE_UPLOAD_MIB:-16}"
export ONEKEE_UPLOAD_US="${ONEKEE_UPLOAD_US:-2000}"
export ONEKEE_TERRAIN_TIMINGS="${ONEKEE_TERRAIN_TIMINGS:-1}"
exec cargo run --release -p one-thousand-electric-eye-desktop -- "$@"
