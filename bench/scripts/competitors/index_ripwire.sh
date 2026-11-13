#!/bin/bash
# index_ripwire.sh REPO_DIR OUT_JSON
# Cold-parses REPO_DIR with ripwire and writes both committable cache families to an external directory:
#   $WORK_DIR/ripwire/<key>/idx.lean.ripwirecache  (map / defs / callers / callees)
#   $WORK_DIR/ripwire/<key>/idx.rich.ripwirecache  (--for / --uses / --metrics)
# The repository is only read. ripwire has no thread-count flag (it uses every core), so the run is
# recorded as threads_requested="uncapped".
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/_common.sh" || exit 1
index_args "$@"
IDX="$WORK_DIR/ripwire/$KEY"; rm -rf "$IDX"; mkdir -p "$IDX"
VER="$(sbx ripwire --version 2>&1 | head -1)"
MEASURE_THREADS=uncapped \
measure "$OUT" ripwire "$REPO" "$VER" "$IDX" -- ripwire "$REPO" --no-cache --index-out="$IDX/idx"
