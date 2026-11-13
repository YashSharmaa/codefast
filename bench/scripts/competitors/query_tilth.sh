#!/bin/bash
# query_tilth.sh REPO_DIR def|callers|callees|locate ARG        (no index; each call walks the tree)
#   def     -> tilth ARG --scope REPO                 (search; definitions are listed first)
#   callers -> tilth ARG --callers --scope REPO --full   (--full raises the call-site cap from 10 to 100)
#   callees -> tilth grok ARG --scope REPO            (one call: signature, body, callees, callers, tests;
#                                                      tilth has no callees-only verb)
#   locate  -> tilth "ARG" --scope REPO               (multi-word text search; literal, not semantic)
# TILTH_FLAGS adds flags, e.g. TILTH_FLAGS="--json", "--budget 2000", "--expand=1".
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/_common.sh" || exit 1
query_args "$@"
export SBX_EXTRA_ENV="RAYON_NUM_THREADS=$BENCH_JOBS"
cd "$REPO" || exit 1
# shellcheck disable=SC2086
case "$KIND" in
  def)     sbx tilth "$ARG" --scope "$REPO" ${TILTH_FLAGS:-} ;;
  callers) sbx tilth "$ARG" --callers --full --scope "$REPO" ${TILTH_FLAGS:-} ;;
  callees) sbx tilth grok "$ARG" --scope "$REPO" ${TILTH_FLAGS:-} ;;
  locate)  sbx tilth "$ARG" --scope "$REPO" ${TILTH_FLAGS:-} ;;
esac
