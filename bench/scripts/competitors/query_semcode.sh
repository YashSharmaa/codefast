#!/bin/bash
# query_semcode.sh REPO_DIR def|callers|callees|locate ARG      (semcode -q, non-interactive)
#   def     -> func ARG        (function or function-like macro, exact name or regex; SEMCODE_DEF=type uses `type ARG`)
#   callers -> callers ARG
#   callees -> calls ARG
#   locate  -> grep ARG        (REGEX over function bodies; semcode has no natural-language entry point
#                               unless vectors were built with `semcode-index --vectors`, then
#                               SEMCODE_LOCATE=vgrep uses `vgrep "ARG"`)
# ANSI colour codes are stripped (semcode colours `grep` output even with NO_COLOR); SEMCODE_KEEP_ANSI=1 keeps them.
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/_common.sh" || exit 1
query_args "$@"
IDX="$WORK_DIR/semcode/$KEY"
[ -d "$IDX/.semcode.db" ] || { echo "no index for $REPO (run index_semcode.sh)" >&2; exit 4; }
case "$KIND" in
  def)     q="${SEMCODE_DEF:-func} $ARG" ;;
  callers) q="callers $ARG" ;;
  callees) q="calls $ARG" ;;
  locate)  if [ "${SEMCODE_LOCATE:-grep}" = vgrep ]; then q="vgrep \"$ARG\""; else q="grep $ARG"; fi ;;
esac
cd "$REPO" || exit 1
if [ -n "${SEMCODE_KEEP_ANSI:-}" ]; then
  sbx semcode -d "$IDX" --git-repo "$REPO" -q "$q"
else
  sbx semcode -d "$IDX" --git-repo "$REPO" -q "$q" | sed $'s/\x1b\\[[0-9;]*[A-Za-z]//g'
  exit "${PIPESTATUS[0]}"
fi
