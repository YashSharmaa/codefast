#!/bin/bash
# index_global.sh REPO_DIR OUT_JSON
# GNU Global tag files in an external dbpath:
#   cd REPO && gtags -f <list> --gtagslabel=native-pygments $WORK_DIR/global/<key>/
# native-pygments = built-in parser for C/C++/Java/PHP/asm/yacc, Pygments plug-in (with universal-ctags
# for definitions) for everything else. Without the explicit file list the Pygments plug-in aborts the
# whole run on binary files ("gtags: unexpected type string" on a gettext .mo file in django).
# The repository is only read. gtags is single-threaded.
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/_common.sh" || exit 1
index_args "$@"
IDX="$WORK_DIR/global/$KEY"; rm -rf "$IDX"; mkdir -p "$IDX/db"
source_file_list "$REPO" "$IDX/files.list"
LABEL="${GTAGS_LABEL:-native-pygments}"
VER="$(sbx gtags --version 2>&1 | head -1)"
cd "$REPO" || exit 1
MEASURE_THREADS=1 MEASURE_EXTRA_JSON="{\"files_listed\":$(wc -l < "$IDX/files.list" | tr -d ' '),\"gtagslabel\":\"$LABEL\"}" \
measure "$OUT" gnu-global "$REPO" "$VER" "$IDX/db" -- \
  gtags -f "$IDX/files.list" --gtagslabel="$LABEL" "$IDX/db"
