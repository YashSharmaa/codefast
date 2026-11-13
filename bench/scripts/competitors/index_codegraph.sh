#!/bin/bash
# index_codegraph.sh REPO_DIR OUT_JSON
# CodeGraph always writes its index to <project>/.codegraph/ (CODEGRAPH_DIR only renames that folder) and,
# when the watcher is off, installs git hooks. So this script indexes a throwaway APFS clone:
#   $WORK_DIR/codegraph/copy-<key>/            (the clone; .codegraph/codegraph.db inside it is the index)
# Only `codegraph init -y` is timed; making the clone is not.
# Daemon and watcher are disabled (CODEGRAPH_NO_DAEMON=1, CODEGRAPH_NO_WATCH=1), telemetry is off
# (DO_NOT_TRACK=1, CODEGRAPH_TELEMETRY=0, and `codegraph telemetry off` persisted in the sandbox HOME).
# Parse and resolve pools are capped at BENCH_JOBS workers.
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/_common.sh" || exit 1
index_args "$@"
COPY="$(fresh_repo_copy "$REPO" codegraph)"
export SBX_EXTRA_ENV="CODEGRAPH_NO_WATCH=1 CODEGRAPH_PARSE_WORKERS=$BENCH_JOBS CODEGRAPH_RESOLVE_WORKERS=$BENCH_JOBS"
VER="$(sbx codegraph version 2>&1 | head -1)"
cd "$COPY" || exit 1
MEASURE_PS_MATCH="$TOOLS_DIR/codegraph/pkg/" MEASURE_EXTRA_JSON="{\"indexed_copy\":\"$COPY\"}" \
measure "$OUT" codegraph "$REPO" "$VER" "$COPY/.codegraph" -- codegraph init -y "$COPY"
rc=$?
# CODEGRAPH_NO_WATCH makes init install post-commit/post-merge/post-checkout hooks in the CLONE's .git;
# remove them so nothing ever re-syncs in the background.
for h in post-commit post-merge post-checkout; do
  f="$COPY/.git/hooks/$h"; [ -f "$f" ] && grep -q 'codegraph sync hook' "$f" && rm -f "$f"
done
pgrep -f "$TOOLS_DIR/codegraph/pkg/" >/dev/null && echo "warning: a codegraph process is still running" >&2
exit $rc
