#!/bin/bash
# query_ast-grep.sh REPO_DIR def|callers|callees|locate ARG
# No index: each call is one `ast-grep scan --inline-rules <rules> REPO` pass with the rule set that
# _astgrep_rules.py generates for ARG (Python, C, C++, Go, Rust, Java, TypeScript, TSX, JavaScript).
#   def     -> nodes defining ARG (function/method/macro/class/struct/type)
#   callers -> call SITES of ARG (the enclosing function is not named by ast-grep)
#   callees -> call sites lexically inside a function named ARG
#   locate  -> unsupported (ast-grep matches structure, not free text)      exit 3
# Output: one JSON object per match (--json=stream). ASTGREP_OUTPUT=short gives file:line:col lines,
# ASTGREP_OUTPUT=rich the default diagnostic with code frames.
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/_common.sh" || exit 1
query_args "$@"
[ "$KIND" = locate ] && unsupported "ast-grep has no free-text query"
RULES="$(/usr/bin/python3 -I "$(dirname "${BASH_SOURCE[0]}")/_astgrep_rules.py" "$KIND" "$ARG")" || exit 1
case "${ASTGREP_OUTPUT:-json}" in
  json)  fmt=(--json=stream) ;;
  short) fmt=(--report-style short) ;;
  rich)  fmt=(--report-style rich) ;;
  *) echo "ASTGREP_OUTPUT must be json, short or rich" >&2; exit 2 ;;
esac
cd "$REPO" || exit 1
sbx ast-grep scan --inline-rules "$RULES" -j "$BENCH_JOBS" "${fmt[@]}" .
