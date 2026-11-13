#!/bin/bash
# Rebuild every call-graph oracle from scratch.
#
#   bench/scripts/oracles/build_all.sh [gitleaks] [syft] [zod] [redis]
#
# Prerequisites (see bench/oracles/README.md): go (brew install go), node + npm,
# python3, Xcode command line tools (clang, libclang.dylib), git.
# Everything else is fetched into bench/oracles/.work/ (git-ignored) with HOME
# redirected there, so nothing under the real home directory is written.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
. "$HERE/env.sh"
TARGETS=("$@"); [ ${#TARGETS[@]} -eq 0 ] && TARGETS=(gitleaks syft zod redis)
want() { local t; for t in "${TARGETS[@]}"; do [ "$t" = "$1" ] && return 0; done; return 1; }

bash "$HERE/fetch_repos.sh"

# ---- Go: gitleaks, syft ------------------------------------------------------
if want gitleaks || want syft; then
  mkdir -p "$ORACLE_WORK/bin"
  ( cd "$HERE/go" && go build -o "$ORACLE_WORK/bin/go-oracle" . )
  # One fixed target platform, so the answer key does not depend on the host.
  export GOOS=linux GOARCH=amd64 CGO_ENABLED=0
  # The syft run holds the SSA form of the whole dependency graph. Left alone
  # the Go runtime lets it peak at 7.2 GB of RAM; a soft memory limit keeps it
  # under 5 GB (measured: 4.9 GB) for about 10 s more wall time. The output
  # does not depend on these two settings.
  export GOMEMLIMIT="${GOMEMLIMIT:-4500MiB}" GOGC="${GOGC:-40}"
  for r in gitleaks syft; do
    want "$r" || continue
    ( cd "$ORACLE_REPOS/$r" && go mod download &&
      "$ORACLE_WORK/bin/go-oracle" -repo . -name "$r" -commit "$(git rev-parse HEAD)" -out "$ORACLE_OUT/$r" )
  done
  # with-tests variant: gitleaks only (syft with tests needs well over 6 GB of RAM).
  if want gitleaks; then
    ( cd "$ORACLE_REPOS/gitleaks" &&
      "$ORACLE_WORK/bin/go-oracle" -repo . -tests -suffix .with_tests -name gitleaks \
        -commit "$(git rev-parse HEAD)" -out "$ORACLE_OUT/gitleaks" )
  fi
  unset GOOS GOARCH CGO_ENABLED GOMEMLIMIT GOGC
fi

# ---- TypeScript: zod ---------------------------------------------------------
if want zod; then
  ( cd "$HERE/ts" && npm ci --ignore-scripts --no-audit --no-fund >/dev/null )
  Z="$ORACLE_REPOS/zod"
  ( cd "$HERE/ts" && node oracle.mjs -repo "$Z" -subdir packages/zod/src \
      -tsconfig "$Z/packages/zod/tsconfig.json" -name zod -commit "$(git -C "$Z" rev-parse HEAD)" \
      -out "$ORACLE_OUT/zod" )
fi

# ---- C: redis ----------------------------------------------------------------
if want redis; then
  [ -x "$ORACLE_WORK/venv/bin/python" ] || python3 -m venv "$ORACLE_WORK/venv"
  "$ORACLE_WORK/venv/bin/pip" install --quiet --no-deps -r "$HERE/c/requirements.txt"
  bash "$HERE/c/build_compdb.sh"
  LIBCLANG="${LIBCLANG:-$(dirname "$(dirname "$(xcrun --find clang)")")/lib/libclang.dylib}"
  ( cd "$ORACLE_WORK" && "$ORACLE_WORK/venv/bin/python" -I "$HERE/c/extract.py" \
      --build-root "$ORACLE_WORK/redis-build" --out "$ORACLE_OUT/redis" --name redis \
      --libclang "$LIBCLANG" --jobs "${JOBS:-6}" --exclude-prefix tests/ )
fi

# ---- checks --------------------------------------------------------------------
# Mechanical invariants over every row (verify/invariants.py), then the scorer
# self-test. The slower second-derivation cross-checks are in verify/run_all.sh.
for r in "${TARGETS[@]}"; do
  src="$ORACLE_REPOS/$r"; [ "$r" = redis ] && src="$ORACLE_WORK/redis-build"
  python3 -I "$HERE/verify/invariants.py" "$ORACLE_OUT/$r" "$src"
  [ "$r" = gitleaks ] && python3 -I "$HERE/verify/invariants.py" "$ORACLE_OUT/$r" "$src" --variant .with_tests
  python3 -I "$HERE/selftest_score.py" "$ORACLE_OUT/$r"
done
( cd "$ORACLE_OUT" && shasum -a 256 */edges*.jsonl */functions*.jsonl */unresolved_sites*.jsonl > SHA256SUMS && cat SHA256SUMS )
