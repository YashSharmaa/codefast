#!/bin/bash
# query_global.sh REPO_DIR def|callers|callees|locate ARG
#   def     -> global -x ARG           (definitions, cxref format)
#   callers -> global -rx ARG          (REFERENCE lines of ARG: call sites and imports; the enclosing
#                                       function is not named, so this is "references", not "callers")
#   callees -> unsupported (GNU Global has no outgoing-call query)   exit 3
#   locate  -> global -gx ARG          (grep-style pattern over the indexed files; -i with GLOBAL_ICASE=1)
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/_common.sh" || exit 1
query_args "$@"
DB="$WORK_DIR/global/$KEY/db"
[ -f "$DB/GTAGS" ] || { echo "no index for $REPO (run index_global.sh)" >&2; exit 4; }
export SBX_EXTRA_ENV="GTAGSROOT=$REPO GTAGSDBPATH=$DB GTAGSLABEL=${GTAGS_LABEL:-native-pygments}"
ic=(); [ -n "${GLOBAL_ICASE:-}" ] && ic=(-i)
cd "$REPO" || exit 1
case "$KIND" in
  def)     sbx global -x "$ARG" ;;
  callers) sbx global -rx "$ARG" ;;
  callees) unsupported "GNU Global has no callees query" ;;
  locate)  sbx global -gx "${ic[@]}" -e "$ARG" ;;
esac
