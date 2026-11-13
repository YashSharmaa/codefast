#!/bin/bash
# index_gortex.sh REPO_DIR OUT_JSON
# Starts an isolated gortex daemon (see _gortex.sh), tracks a throwaway APFS clone of the repo and waits
# until the graph is queryable, then stops the daemon:
#   gortex daemon start --detach --backend-path <store>
#   gortex track <clone> --name <key> --wait --wait-timeout 0        <- this command is what is timed
#   gortex daemon stop
# Peak RSS is the sampled sum over the daemon and its children (the CLI is a thin client).
# GORTEX_LSP=auto (default, as shipped): gortex auto-registers every language server it finds on PATH
#   (here clangd, pyright, sourcekit-lsp) and uses them to upgrade call edges while indexing. The JSON
#   lists which servers it actually started. Results therefore depend on what is installed.
# GORTEX_LSP=off: sets GORTEX_LSP_RESOLVER=0 for the daemon and writes `semantic: {enabled: false}` to
#   .gortex.yaml in the clone, so only the tree-sitter graph is built (no language server is spawned).
# Threads: GOMAXPROCS=BENCH_JOBS for the daemon (language servers are not capped by it).
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/_common.sh" || exit 1
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/_gortex.sh" || exit 1
index_args "$@"
LSP="${GORTEX_LSP:-auto}"
[ "$LSP" = off ] && GX_EXTRA_ENV="GORTEX_LSP_RESOLVER=0"
COPY="$(fresh_repo_copy "$REPO" gortex)"
rm -rf "$WORK_DIR/gortex/$KEY" "$SHORT_ROOT/gortex/$KEY"
gortex_setup "$REPO" "$KEY"
if [ "$LSP" = off ] && [ ! -e "$COPY/.gortex.yaml" ]; then
  # per-repo config lives in the tracked root; the clone is throwaway, so it is safe to add it there
  printf 'semantic:\n  enabled: false\n' > "$COPY/.gortex.yaml"
fi
VER="$(sbx "$GX_BIN" version 2>&1 | head -1)"
gortex_start || exit 1
cd "$SANDBOX_HOME" || exit 1
MEASURE_PS_MATCH="backend-path=$GX_DIR/store/store.sqlite" \
measure "$OUT.tmp" gortex "$REPO" "$VER" "$GX_DIR/store" -- \
  "$GX_BIN" track "$COPY" --name "$KEY" --wait --wait-timeout 0
rc=$?
# which language servers did the daemon start, and what does it report about itself?
lsps=$(grep -o '"LSP router provider startup complete","spec":"[^"]*"' "$GX_RUN/cache/gortex/daemon.log" 2>/dev/null | sed 's/.*"spec":"//; s/"//' | sort -u | paste -sd, -)
sbx "$GX_BIN" daemon status > "$OUT.daemon-status.txt" 2>&1
gortex_stop
# sizes are taken again after the daemon has stopped: while it runs the store carries a large WAL
/usr/bin/python3 -I "$(dirname "${BASH_SOURCE[0]}")/_gortex_json.py" "$OUT.tmp" "$OUT" "$LSP" "$lsps" "$COPY" \
  "$(tree_bytes "$GX_DIR/store")" "$(tree_disk_bytes "$GX_DIR/store")"
mv -f "$OUT.tmp.log" "$OUT.log" 2>/dev/null; rm -f "$OUT.tmp"
exit $rc
