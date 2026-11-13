#!/bin/bash
# index_ctags.sh REPO_DIR OUT_JSON
# universal-ctags over the shared source-file list (see source_file_list in _common.sh):
#   cd REPO && ctags -L <list> -f $WORK_DIR/ctags/<key>/tags --fields=+niaSK --extras=+q
# The tags file is external; the repository is only read. ctags is single-threaded.
# CTAGS_EXTRA_ARGS adds flags (for the kernel: "--c-kinds=+px --langmap=c:+.h" plus the regex_c rules
# of scripts/tags.sh if you want macro-defined symbols).
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/_common.sh" || exit 1
index_args "$@"
IDX="$WORK_DIR/ctags/$KEY"; rm -rf "$IDX"; mkdir -p "$IDX"
source_file_list "$REPO" "$IDX/files.list"
VER="$(sbx ctags --version 2>&1 | head -1)"
cd "$REPO" || exit 1
# shellcheck disable=SC2086
MEASURE_THREADS=1 MEASURE_EXTRA_JSON="{\"files_listed\":$(wc -l < "$IDX/files.list" | tr -d ' ')}" \
measure "$OUT" universal-ctags "$REPO" "$VER" "$IDX/tags" -- \
  ctags -L "$IDX/files.list" -f "$IDX/tags" --fields=+niaSK --extras=+q ${CTAGS_EXTRA_ARGS:-}
