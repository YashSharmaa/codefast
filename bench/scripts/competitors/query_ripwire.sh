#!/bin/bash
# query_ripwire.sh REPO_DIR def|callers|callees|locate ARG
# Uses the caches written by index_ripwire.sh (falls back to ripwire's own TMPDIR cache / a cold parse
# when they are missing; ripwire then says so on stderr).
#   def     -> ripwire REPO --defs=ARG
#   callers -> ripwire REPO --callers=ARG
#   callees -> ripwire REPO --callees=ARG
#   locate  -> ripwire REPO --for=ARG          (task lens: ranked signatures + bodies)
# RIPWIRE_FLAGS adds flags, e.g. RIPWIRE_FLAGS="--json" or "--token-budget=2K".
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/_common.sh" || exit 1
query_args "$@"
IDX="$WORK_DIR/ripwire/$KEY/idx"
lean=(); rich=()
[ -f "$IDX.lean.ripwirecache" ] && lean=(--cache="$IDX.lean.ripwirecache")
[ -f "$IDX.rich.ripwirecache" ] && rich=(--cache="$IDX.rich.ripwirecache")
# shellcheck disable=SC2086
case "$KIND" in
  def)     sbx ripwire "$REPO" "${lean[@]}" --defs="$ARG" ${RIPWIRE_FLAGS:-} ;;
  callers) sbx ripwire "$REPO" "${lean[@]}" --callers="$ARG" ${RIPWIRE_FLAGS:-} ;;
  callees) sbx ripwire "$REPO" "${lean[@]}" --callees="$ARG" ${RIPWIRE_FLAGS:-} ;;
  locate)  sbx ripwire "$REPO" "${rich[@]}" --for="$ARG" ${RIPWIRE_FLAGS:-} ;;
esac
