#!/bin/bash
# Independent verification of the oracles (see bench/oracles/README.md,
# "Independent verification"). Run after build_all.sh.
#
#   bench/scripts/oracles/verify/run_all.sh [gitleaks] [syft] [zod] [redis]
#
# 1. invariants.py     mechanical invariants over every row
# 2. second derivation of the compiler-resolved edges by a different route:
#      Go          go/crosscheck  (go/ast + go/types, no SSA)        vs static rows
#      TypeScript  ts/crosscheck.mjs (symbol table, not signatures)  vs static rows
#      C           c_crosscheck.py (clang's own CallGraph, via the
#                  static analyzer's debug.DumpCallGraph)            vs static + macro rows
# 3. sample.py         the seeded hand-check sample (printed to .work/verify/)
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
. "$HERE/../env.sh"
TARGETS=("$@"); [ ${#TARGETS[@]} -eq 0 ] && TARGETS=(gitleaks syft zod redis)
OUT="$ORACLE_WORK/verify"; mkdir -p "$OUT" "$ORACLE_WORK/bin"
SEED="${SEED:-4051977}"
for r in "${TARGETS[@]}"; do
  src="$ORACLE_REPOS/$r"; [ "$r" = redis ] && src="$ORACLE_WORK/redis-build"
  echo "=== $r"
  python3 -I "$HERE/invariants.py" "$ORACLE_OUT/$r" "$src"
  case "$r" in
    gitleaks|syft)
      ( cd "$HERE/../go" && go build -o "$ORACLE_WORK/bin/go-crosscheck" ./crosscheck/ )
      ( cd "$src" && GOOS=linux GOARCH=amd64 CGO_ENABLED=0 GOFLAGS=-p=4 "$ORACLE_WORK/bin/go-crosscheck" -repo . > "$OUT/ast-$r.jsonl" )
      python3 -I "$HERE/go_crosscheck.py" "$ORACLE_OUT/$r" "$OUT/ast-$r.jsonl" ;;
    zod)
      ( cd "$HERE/../ts" && node crosscheck.mjs -repo "$src" -subdir packages/zod/src \
          -tsconfig "$src/packages/zod/tsconfig.json" -oracle "$ORACLE_OUT/zod" > "$OUT/ts-crosscheck-zod.json" )
      python3 -I -c "import json,sys; d=json.load(open(sys.argv[1])); [print(k, '=', json.dumps(v)) for k, v in d.items() if 'example' not in k]" "$OUT/ts-crosscheck-zod.json" ;;
    redis)
      python3 -I "$HERE/c_crosscheck.py" "$ORACLE_OUT/redis" "$src" --jobs "${JOBS:-4}" ;;
  esac
  python3 -I "$HERE/sample.py" "$ORACLE_OUT/$r" "$src" --seed "$SEED" > "$OUT/sample-$r.txt"
  echo "sample written to $OUT/sample-$r.txt (seed $SEED)"
done
