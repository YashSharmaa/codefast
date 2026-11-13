#!/bin/bash
# index_ast-grep.sh REPO_DIR OUT_JSON
# ast-grep has no index: every query re-parses the tree. To have a comparable "cost of being ready"
# number this script times one full structural pass over the repository (the `def` rule set for a name
# that matches nothing), which is what any ast-grep query pays. index_bytes is 0.
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/_common.sh" || exit 1
index_args "$@"
VER="$(sbx ast-grep --version 2>&1 | head -1)"
RULES="$(/usr/bin/python3 -I "$(dirname "${BASH_SOURCE[0]}")/_astgrep_rules.py" def __codefast_bench_no_such_symbol__)"
cd "$REPO" || exit 1
MEASURE_EXTRA_JSON='{"index":"none","note":"no index; this is one full scan pass, paid again by every query"}' \
measure "$OUT" ast-grep "$REPO" "$VER" "" -- \
  ast-grep scan --inline-rules "$RULES" -j "$BENCH_JOBS" --json=stream .
