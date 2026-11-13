#!/bin/bash
# index_tilth.sh REPO_DIR OUT_JSON
# tilth builds no index. To have a comparable number this script times one cold symbol search over the
# whole tree (a name that matches nothing), which is what every tilth query pays. index_bytes is 0.
# tilth has no thread flag; RAYON_NUM_THREADS is set to BENCH_JOBS in case it uses rayon (not verified
# to have an effect), so the run is recorded as threads_requested="uncapped".
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/_common.sh" || exit 1
index_args "$@"
VER="$(sbx tilth --version 2>&1 | head -1)"
export SBX_EXTRA_ENV="RAYON_NUM_THREADS=$BENCH_JOBS"
cd "$REPO" || exit 1
MEASURE_THREADS=uncapped MEASURE_EXTRA_JSON='{"index":"none","note":"no index; this is one cold whole-tree symbol search, paid again by every query"}' \
measure "$OUT" tilth "$REPO" "$VER" "" -- \
  tilth __codefast_bench_no_such_symbol__ --scope "$REPO"
