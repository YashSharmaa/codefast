#!/bin/bash
# index_semcode.sh REPO_DIR OUT_JSON
# semcode-index into an external LanceDB directory (the default would be REPO/.semcode.db):
#   cd REPO && semcode-index -s REPO -d $WORK_DIR/semcode/<key> -e <extensions> -j BENCH_JOBS
# REPO must be a git checkout (rows are keyed by git blob hash). The repository is only read.
# Extensions default to everything semcode can parse (c,h,cc,cpp,cxx,hh,hpp,hxx,rs,py,zig); the tool's own
# default is c,h,rs,zig. Override with SEMCODE_EXTENSIONS. No vectors, no lore, no commit history.
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/_common.sh" || exit 1
index_args "$@"
IDX="$WORK_DIR/semcode/$KEY"; rm -rf "$IDX"; mkdir -p "$IDX"
EXT="${SEMCODE_EXTENSIONS:-c,h,cc,cpp,cxx,hh,hpp,hxx,rs,py,zig}"
VER="semcode $(sed -n 's/^version = "\(.*\)"/\1/p' "$TOOLS_DIR/semcode/src/Cargo.toml" | head -1) git $(git -C "$TOOLS_DIR/semcode/src" rev-parse --short=12 HEAD 2>/dev/null)"
cd "$REPO" || exit 1
MEASURE_EXTRA_JSON="{\"extensions\":\"$EXT\"}" \
measure "$OUT" semcode "$REPO" "$VER" "$IDX" -- \
  semcode-index -s "$REPO" -d "$IDX" -e "$EXT" -j "$BENCH_JOBS"
