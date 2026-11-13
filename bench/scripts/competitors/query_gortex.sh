#!/bin/bash
# query_gortex.sh REPO_DIR def|callers|callees|locate ARG
# Needs index_gortex.sh to have run for REPO_DIR. If the repo's daemon is not running it is started
# (warm start from the on-disk store) and stopped again after the query; a daemon that was already
# running (gortex_daemon.sh start REPO_DIR) is left running, which is the way to measure warm latency.
#   def     -> gortex query symbol ARG
#   callers -> gortex query callers ARG --depth 1
#   callees -> gortex query calls <id> --depth 1 for every function/method whose name is exactly ARG
#              (`query calls` only accepts a graph id, so ids are resolved with `query symbol --format json`)
#   locate  -> gortex explore "ARG"             (smart_context working set, JSON)
# GORTEX_LIMIT (default 200) caps result nodes; GORTEX_FORMAT=json for the query verbs.
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/_common.sh" || exit 1
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/_gortex.sh" || exit 1
query_args "$@"
[ "${GORTEX_LSP:-auto}" = off ] && GX_EXTRA_ENV="GORTEX_LSP_RESOLVER=0"
gortex_setup "$REPO" "$KEY"
[ -f "$GX_DIR/store/store.sqlite" ] || { echo "no index for $REPO (run index_gortex.sh)" >&2; exit 4; }
started=0
if ! gortex_running; then gortex_start || exit 1; started=1; fi
cd "$SANDBOX_HOME" || exit 1
G=("$GX_BIN" --loading-wait 300s)
lim=(--limit "${GORTEX_LIMIT:-200}"); fmt=(--format "${GORTEX_FORMAT:-text}")
case "$KIND" in
  def)     sbx "${G[@]}" query symbol "$ARG" --index "$GX_COPY" "${lim[@]}" "${fmt[@]}"; rc=$? ;;
  callers) sbx "${G[@]}" query callers "$ARG" --index "$GX_COPY" --depth 1 "${lim[@]}" "${fmt[@]}"; rc=$? ;;
  callees)
    ids=$(sbx "${G[@]}" query symbol "$ARG" --index "$GX_COPY" --limit 200 --format json 2>/dev/null \
          | /usr/bin/python3 -I "$(dirname "${BASH_SOURCE[0]}")/_gortex_ids.py" "$ARG")
    rc=0
    if [ -z "$ids" ]; then echo "no function or method named exactly '$ARG'" >&2; rc=1; fi
    while IFS= read -r id; do
      [ -n "$id" ] || continue
      sbx "${G[@]}" query calls "$id" --index "$GX_COPY" --depth 1 "${lim[@]}" "${fmt[@]}" || rc=$?
    done <<< "$ids" ;;
  locate)  sbx "${G[@]}" explore "$ARG" --index "$GX_COPY"; rc=$? ;;
esac
[ "$started" = 1 ] && gortex_stop
exit $rc
