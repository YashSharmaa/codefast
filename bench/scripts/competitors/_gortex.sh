#!/bin/bash
# Shared gortex plumbing (sourced by index_gortex.sh, query_gortex.sh, gortex_daemon.sh).
# gortex only answers through a long-lived daemon. Each benchmark repo gets its own isolated daemon
# "universe" so runs never share state:
#   config  $WORK_DIR/gortex/<key>/cfg/gortex/config.yaml      (XDG_CONFIG_HOME)
#   store   $WORK_DIR/gortex/<key>/store/store.sqlite          (--backend-path; this is the index)
#   runtime $SHORT_ROOT/gortex/<key>/cache/gortex/daemon.sock  (XDG_CACHE_HOME: socket, pid, logs; short
#           path because unix sockets are limited to 103 bytes on macOS; recreated on demand)
#   data    sandbox HOME/.local/share/gortex                   (shared: the potion-code-16M-v2 static
#           embedding model gortex downloads on first daemon start, ~32 MB)
# The daemon indexes a throwaway APFS clone of the repo, never the benchmark repo itself.
gortex_setup() { # REPO KEY
  GX_COPY="$(existing_repo_copy "$1" gortex)"
  GX_DIR="$WORK_DIR/gortex/$2"
  GX_RUN="$SHORT_ROOT/gortex/$2"
  GX_BIN="$TOOLS_DIR/gortex/bin/gortex"
  mkdir -p "$SHORT_ROOT" && chmod 700 "$SHORT_ROOT"
  mkdir -p "$GX_DIR/cfg" "$GX_DIR/store" "$GX_RUN/cache"
  local sock="$GX_RUN/cache/gortex/daemon.sock"
  if [ "${#sock}" -gt 100 ]; then echo "gortex socket path too long (${#sock} bytes): $sock; set BENCH_SHORT_ROOT" >&2; exit 2; fi
  export SBX_EXTRA_ENV="XDG_CONFIG_HOME=$GX_DIR/cfg XDG_CACHE_HOME=$GX_RUN/cache GOMAXPROCS=$BENCH_JOBS${GX_EXTRA_ENV:+ $GX_EXTRA_ENV}"
}
gortex_running() { sbx "$GX_BIN" daemon status >/dev/null 2>&1; }
gortex_start() {
  gortex_running && return 0
  sbx "$GX_BIN" daemon start --detach --backend-path "$GX_DIR/store/store.sqlite" >"$GX_RUN/start.log" 2>&1
  local i; for i in $(seq 1 100); do gortex_running && return 0; sleep 0.2; done
  echo "gortex daemon did not come up; see $GX_RUN/start.log and $GX_RUN/cache/gortex/daemon.log" >&2; return 1
}
gortex_stop() {
  sbx "$GX_BIN" daemon stop >/dev/null 2>&1
  local i; for i in $(seq 1 50); do pgrep -f "backend-path=$GX_DIR/store/store.sqlite" >/dev/null || return 0; sleep 0.2; done
  echo "warning: gortex daemon for $GX_DIR still running" >&2; return 1
}
