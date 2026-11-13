#!/bin/bash
# Final smoke pass: every tool on django, sequentially. Fast indexers are timed 3 times (cold index each
# time; the scripts wipe the previous index), heavy ones once.
BENCH="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
A=$BENCH/tools/_audit; D=$BENCH/scripts/competitors
O=$BENCH/tools/_smoke; R=$BENCH/repos/django
mkdir -p $O/runs
for t in ripwire codebase-memory-mcp codegraph semcode ctags cscope global tilth ast-grep; do
  for i in 1 2; do $D/index_$t.sh $R $O/runs/$t.run$i.json >/dev/null 2>&1; echo "$t run$i rc=$?"; done
  echo "##### $t"; $A/smoke.sh $t; cp $O/$t.index.json $O/runs/$t.run3.json
done
echo "##### gortex (GORTEX_LSP=off)"; GORTEX_LSP=off $D/index_gortex.sh $R $O/gortex-lsp-off.index.json; echo "rc=$?"
echo "##### gortex (auto)"; $A/smoke.sh gortex
echo "leftover processes: $(pgrep -fl 'tools/(gortex|codebase-memory-mcp|codegraph|ripwire|semcode|tilth)/' | wc -l | tr -d ' ')"
