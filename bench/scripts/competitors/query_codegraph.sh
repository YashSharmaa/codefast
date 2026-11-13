#!/bin/bash
# query_codegraph.sh REPO_DIR def|callers|callees|locate ARG
# Queries the clone that index_codegraph.sh indexed for REPO_DIR.
#   def     -> codegraph query ARG              (symbol search; CODEGRAPH_KIND=function adds --kind)
#   callers -> codegraph callers ARG --limit N
#   callees -> codegraph callees ARG --limit N
#   locate  -> codegraph explore ARG            (same output as the codegraph_explore MCP tool)
# CODEGRAPH_LIMIT (default 100 for callers/callees, 10 for def); CODEGRAPH_JSON=1 adds --json where supported.
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/_common.sh" || exit 1
query_args "$@"
COPY="$(existing_repo_copy "$REPO" codegraph)"
[ -f "$COPY/.codegraph/codegraph.db" ] || { echo "no index for $REPO (run index_codegraph.sh)" >&2; exit 4; }
export SBX_EXTRA_ENV="CODEGRAPH_NO_WATCH=1"
js=(); [ -n "${CODEGRAPH_JSON:-}" ] && js=(--json)
kd=(); [ -n "${CODEGRAPH_KIND:-}" ] && kd=(--kind "$CODEGRAPH_KIND")
cd "$COPY" || exit 1
case "$KIND" in
  def)     sbx codegraph query "$ARG" -p "$COPY" --limit "${CODEGRAPH_LIMIT:-10}" "${kd[@]}" "${js[@]}" ;;
  callers) sbx codegraph callers "$ARG" -p "$COPY" --limit "${CODEGRAPH_LIMIT:-100}" "${js[@]}" ;;
  callees) sbx codegraph callees "$ARG" -p "$COPY" --limit "${CODEGRAPH_LIMIT:-100}" "${js[@]}" ;;
  locate)  sbx codegraph explore -p "$COPY" "$ARG" ;;
esac
