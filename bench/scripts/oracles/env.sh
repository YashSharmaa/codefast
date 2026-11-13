# Source this file: `. bench/scripts/oracles/env.sh`
# Sandboxed environment for building the call-graph oracles. Nothing here
# writes outside bench/ : HOME is redirected so that go, npm and pip keep their
# caches, telemetry and config files under bench/oracles/.work/home.
ORACLE_SCRIPTS="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")" && pwd)"
BENCH_DIR="$(cd "$ORACLE_SCRIPTS/../.." && pwd)"
export ORACLE_SCRIPTS BENCH_DIR
export ORACLE_OUT="$BENCH_DIR/oracles"
export ORACLE_WORK="$ORACLE_OUT/.work"
export ORACLE_REPOS="$BENCH_DIR/repos"
mkdir -p "$ORACLE_WORK/home" "$ORACLE_WORK/tmp"
[ -f "$ORACLE_WORK/.gitignore" ] || printf '*\n' > "$ORACLE_WORK/.gitignore"

export REAL_HOME="${REAL_HOME:-$HOME}"
export HOME="$ORACLE_WORK/home"
export TMPDIR="$ORACLE_WORK/tmp"

# Go: module cache, build cache and telemetry all live in the sandbox HOME.
export GOPATH="$HOME/go"
export GOMODCACHE="$GOPATH/pkg/mod"
export GOCACHE="$HOME/go-build-cache"
export GOTOOLCHAIN=local
export GOMAXPROCS="${GOMAXPROCS:-6}"
export GOTELEMETRY=off
export GOPROXY="https://proxy.golang.org,direct"

# npm / pip: caches in the sandbox, no update checks, no funding/audit calls.
export npm_config_cache="$HOME/.npm"
export npm_config_update_notifier=false
export npm_config_fund=false
export npm_config_audit=false
export PIP_CACHE_DIR="$HOME/.pip-cache"
export PIP_DISABLE_PIP_VERSION_CHECK=1
export PYTHONDONTWRITEBYTECODE=1

export PATH="/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin"
