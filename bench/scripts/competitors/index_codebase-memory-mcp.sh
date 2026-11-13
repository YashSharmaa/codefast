#!/bin/bash
# index_codebase-memory-mcp.sh REPO_DIR OUT_JSON
# Full index (mode=full unless CBM_MODE=moderate|fast) with codebase-memory-mcp's one-shot CLI.
# The repository is only read (persistence=false); the SQLite graph goes to an external cache dir:
#   $SHORT_ROOT/cbm/<key>/      (CBM_CACHE_DIR)
# It cannot live under bench/tools on this machine: the tool refuses any cache root that has a
# world-writable ancestor, and ~/Documents is mode 777 here. /private/tmp is wiped on reboot, so
# re-index after a restart.
# The CLI is a thin client: the index runs in a temporary daemon + supervised worker that exit with
# the command, so peak RSS comes from the process-tree sampler, not from /usr/bin/time.
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/_common.sh" || exit 1
index_args "$@"
BIN="$TOOLS_DIR/codebase-memory-mcp/bin/codebase-memory-mcp"
CACHE="$SHORT_ROOT/cbm/$KEY"
mkdir -p "$SHORT_ROOT" && chmod 700 "$SHORT_ROOT"
rm -rf "$CACHE"; mkdir -p "$SHORT_ROOT/cbm"
MODE="${CBM_MODE:-full}"
export SBX_EXTRA_ENV="CBM_CACHE_DIR=$CACHE CBM_WORKERS=$BENCH_JOBS"
VER="$(sbx "$BIN" --version 2>&1 | head -1)"
cd "$SHORT_ROOT" || exit 1
MEASURE_PS_MATCH="$BIN" MEASURE_EXTRA_JSON="{\"mode\":\"$MODE\",\"project\":\"$KEY\"}" \
measure "$OUT" codebase-memory-mcp "$REPO" "$VER" "$CACHE" -- \
  "$BIN" cli --progress index_repository --repo-path "$REPO" --name "$KEY" --mode "$MODE"
rc=$?
# the temporary daemon exits on its own; make sure nothing is left behind
for _ in 1 2 3 4 5 6 7 8 9 10; do pgrep -f "$BIN" >/dev/null || break; sleep 0.5; done
pgrep -f "$BIN" >/dev/null && echo "warning: codebase-memory-mcp process still running" >&2
exit $rc
