#!/bin/bash
# gortex_daemon.sh start|stop|status REPO_DIR
# Keep the per-repo gortex daemon warm across many query_gortex.sh calls (they leave a daemon that was
# already running alone). ALWAYS run `stop` when the batch is done.
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/_common.sh" || exit 1
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/_gortex.sh" || exit 1
[ $# -eq 2 ] && [ -d "$2" ] || { echo "usage: $(basename "$0") start|stop|status REPO_DIR" >&2; exit 2; }
REPO="$(abs_dir "$2")"; KEY="$(repo_key "$2")"
[ "${GORTEX_LSP:-auto}" = off ] && GX_EXTRA_ENV="GORTEX_LSP_RESOLVER=0"
gortex_setup "$REPO" "$KEY"
case "$1" in
  start)  gortex_start && sbx "$GX_BIN" daemon status | head -12 ;;
  stop)   gortex_stop && echo stopped ;;
  status) sbx "$GX_BIN" daemon status ;;
  *) echo "usage: $(basename "$0") start|stop|status REPO_DIR" >&2; exit 2 ;;
esac
