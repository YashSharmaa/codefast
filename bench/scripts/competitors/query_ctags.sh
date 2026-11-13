#!/bin/bash
# query_ctags.sh REPO_DIR def|callers|callees|locate ARG
#   def     -> readtags -t TAGS -e -n ARG      (exact tag name, extension fields, line numbers)
#   callers -> unsupported (a tags file holds definitions only)          exit 3
#   callees -> unsupported                                               exit 3
#   locate  -> unsupported (no free-text entry; readtags can only prefix/regex-match tag NAMES) exit 3
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/_common.sh" || exit 1
query_args "$@"
TAGS="$WORK_DIR/ctags/$KEY/tags"
[ -f "$TAGS" ] || { echo "no index for $REPO (run index_ctags.sh)" >&2; exit 4; }
case "$KIND" in
  def) sbx readtags -t "$TAGS" -e -n "$ARG" ;;
  *)   unsupported "universal-ctags has no $KIND query" ;;
esac
