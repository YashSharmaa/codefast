#!/bin/bash
# index_cscope.sh REPO_DIR OUT_JSON
# cscope cross-reference with the inverted index, kernel mode (no /usr/include):
#   cd REPO && cscope -b -q -k -i <list> -f $WORK_DIR/cscope/<key>/cscope.out
# External output; the repository is only read. cscope is single-threaded.
# cscope's parser is for C (it reads every listed file as C). On non-C code the "definition", "calling"
# and "called by" answers are unreliable; only the symbol and text searches are meaningful there.
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/_common.sh" || exit 1
index_args "$@"
IDX="$WORK_DIR/cscope/$KEY"; rm -rf "$IDX"; mkdir -p "$IDX"
source_file_list "$REPO" "$IDX/cscope.files"
VER="$(sbx cscope -V 2>&1 | head -1)"
cd "$REPO" || exit 1
MEASURE_THREADS=1 MEASURE_EXTRA_JSON="{\"files_listed\":$(wc -l < "$IDX/cscope.files" | tr -d ' ')}" \
measure "$OUT" cscope "$REPO" "$VER" "$IDX/cscope.out,$IDX/cscope.out.in,$IDX/cscope.out.po" -- \
  cscope -b -q -k -i "$IDX/cscope.files" -f "$IDX/cscope.out"
