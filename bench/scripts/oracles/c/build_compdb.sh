#!/bin/bash
# Build redis in a scratch copy with a recording compiler wrapper and write
# compile_commands.json. Usage: build_compdb.sh  (after `. ../env.sh`)
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
. "$HERE/../env.sh"
REPO="$ORACLE_REPOS/redis"
BUILD="$ORACLE_WORK/redis-build"
JOBS="${JOBS:-6}"

rm -rf "$BUILD"
mkdir -p "$BUILD"
# A clean export of the pinned commit: the shared checkout is never built in.
git -C "$REPO" archive --format=tar HEAD | tar -x -C "$BUILD"
git -C "$REPO" ls-files > "$BUILD/.oracle-git-files"
git -C "$REPO" rev-parse HEAD > "$BUILD/.oracle-commit"

export ORACLE_CC_LOG="$BUILD/.oracle-cc-log.jsonl"
export ORACLE_REAL_CC="$(xcrun --find clang)"
# The toolchain clang (unlike the /usr/bin shim) needs the SDK spelled out.
export SDKROOT="$(xcrun --show-sdk-path)"
: > "$ORACLE_CC_LOG"
# Only the three real binaries: `all` would also build tests/modules.
# MALLOC=libc is the macOS default; stated explicitly so the config is visible.
( cd "$BUILD/src" && make -j"$JOBS" V=1 CC="$HERE/cc_wrapper.py" MALLOC=libc \
    redis-server redis-cli redis-benchmark ) > "$BUILD/.oracle-build.log" 2>&1 \
  || { tail -40 "$BUILD/.oracle-build.log"; exit 1; }

python3 -I "$HERE/make_compdb.py" "$ORACLE_CC_LOG" "$BUILD" "$BUILD/compile_commands.json"
