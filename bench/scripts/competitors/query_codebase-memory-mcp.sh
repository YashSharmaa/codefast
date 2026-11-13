#!/bin/bash
# query_codebase-memory-mcp.sh REPO_DIR def|callers|callees|locate ARG
# Needs index_codebase-memory-mcp.sh to have run for the same REPO_DIR.
#   def     -> cli search_graph --name-pattern '^ARG$'
#   callers -> cli trace_path --function-name ARG --direction inbound  --depth 1 --include-tests true
#   callees -> cli trace_path --function-name ARG --direction outbound --depth 1 --include-tests true
#   locate  -> cli search_graph --query ARG          (BM25 over the graph; the tool's free-text entry)
# CBM_FORMAT=json switches --format; CBM_LIMIT sets --limit (default: tool default).
# Every one-shot CLI call starts and stops a temporary daemon (about 0.5-2 s of overhead per call).
# For warm latency run `sbx.sh codebase-memory-mcp daemon start` with the same CBM_CACHE_DIR first and
# `daemon stop` afterwards.
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/_common.sh" || exit 1
query_args "$@"
BIN="$TOOLS_DIR/codebase-memory-mcp/bin/codebase-memory-mcp"
CACHE="$SHORT_ROOT/cbm/$KEY"
[ -d "$CACHE" ] || { echo "no index for $REPO (run index_codebase-memory-mcp.sh; $CACHE is wiped on reboot)" >&2; exit 4; }
export SBX_EXTRA_ENV="CBM_CACHE_DIR=$CACHE CBM_WORKERS=$BENCH_JOBS"
fmt=(--format "${CBM_FORMAT:-tree}"); lim=()
[ -n "${CBM_LIMIT:-}" ] && lim=(--limit "$CBM_LIMIT")
cd "$SHORT_ROOT" || exit 1
# regex-escape ARG for the anchored name pattern
esc=$(printf '%s' "$ARG" | sed 's/[][\.*^$+?(){}|]/\\&/g')
case "$KIND" in
  def)     sbx "$BIN" cli --quiet search_graph --project "$KEY" --name-pattern "^${esc}\$" "${fmt[@]}" "${lim[@]}" ;;
  callers) sbx "$BIN" cli --quiet trace_path --project "$KEY" --function-name "$ARG" --direction inbound --depth 1 --include-tests true "${fmt[@]}" "${lim[@]}" ;;
  callees) sbx "$BIN" cli --quiet trace_path --project "$KEY" --function-name "$ARG" --direction outbound --depth 1 --include-tests true "${fmt[@]}" "${lim[@]}" ;;
  locate)  sbx "$BIN" cli --quiet search_graph --project "$KEY" --query "$ARG" "${fmt[@]}" "${lim[@]}" ;;
esac
