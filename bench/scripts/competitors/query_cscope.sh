#!/bin/bash
# query_cscope.sh REPO_DIR def|callers|callees|locate ARG      (line-oriented mode, no rebuild)
#   def     -> cscope -d -L -1 ARG     (find this global definition)
#   callers -> cscope -d -L -3 ARG     (find functions calling this function)
#   callees -> cscope -d -L -2 ARG     (find functions called by this function)
#   locate  -> cscope -d -L -4 ARG     (find this text string; literal, not natural language)
# CSCOPE_LOCATE=egrep uses -L -6 (egrep pattern) for locate instead.
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/_common.sh" || exit 1
query_args "$@"
DB="$WORK_DIR/cscope/$KEY/cscope.out"
[ -f "$DB" ] || { echo "no index for $REPO (run index_cscope.sh)" >&2; exit 4; }
cd "$REPO" || exit 1
case "$KIND" in
  def)     n=1 ;;
  callers) n=3 ;;
  callees) n=2 ;;
  locate)  if [ "${CSCOPE_LOCATE:-text}" = egrep ]; then n=6; else n=4; fi ;;
esac
sbx cscope -d -q -k -f "$DB" -L "-$n" "$ARG"
