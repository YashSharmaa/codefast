#!/bin/bash
# Shared helpers for the competitor benchmark scripts. Source this file from bash; do not execute it.
#
# Everything a competitor tool runs goes through `sbx`, which starts the command with an EMPTY
# environment (env -i) and a sandbox HOME. That matters: the interactive shell on this machine exports
# CLAUDE_CONFIG_DIR, and several installers/tools write agent skills/hooks/MCP config into
# $CLAUDE_CONFIG_DIR, $CODEX_HOME, ~/.cursor ... when those exist. With env -i none of them leak in.

# bench/tools; override with BENCH_TOOLS_DIR to relocate the sandbox.
# BASH_SOURCE is empty when this file is sourced from zsh; refuse instead of guessing a location
# (a wrong guess once put the sandbox in the wrong directory).
if [ -n "${BENCH_TOOLS_DIR:-}" ]; then
  TOOLS_DIR="$BENCH_TOOLS_DIR"
elif [ -n "${BASH_SOURCE[0]:-}" ]; then
  _COMP_SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
  TOOLS_DIR="$(cd "$_COMP_SCRIPT_DIR/../.." && pwd)/tools"
else
  echo "_common.sh: source it from bash, or set BENCH_TOOLS_DIR" >&2
  return 1 2>/dev/null || exit 1
fi
case "$TOOLS_DIR" in /*) ;; *) echo "_common.sh: TOOLS_DIR must be absolute: $TOOLS_DIR" >&2; return 1 2>/dev/null || exit 1;; esac
SANDBOX_HOME="$TOOLS_DIR/home"
# Throwaway repo copies and external index directories live here (never inside the benchmark repos).
WORK_DIR="${BENCH_WORK_DIR:-$TOOLS_DIR/_work}"
# Thread cap for long-running index jobs (ground rule: at most 6 on the shared laptop).
BENCH_JOBS="${BENCH_JOBS:-6}"
# Short, private scratch root for tools that reject or cannot use paths under $TOOLS_DIR:
#  - codebase-memory-mcp refuses a cache root with a world-writable ancestor (~/Documents is mode 777 here);
#  - unix-socket paths are limited to 103 bytes on macOS (gortex daemon socket).
# /private/tmp is sticky, the directory itself is created mode 700. It is wiped on reboot.
SHORT_ROOT="${BENCH_SHORT_ROOT:-/private/tmp/codefast-bench-$(id -u)}"

SBX_PATH="$TOOLS_DIR/ripwire/bin:$TOOLS_DIR/codebase-memory-mcp/bin:$TOOLS_DIR/codegraph/bin:$TOOLS_DIR/semcode/bin:$TOOLS_DIR/gortex/bin:$TOOLS_DIR/tilth/root/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin"

# sbx CMD ARGS... : run CMD in the sandbox environment.
# SBX_EXTRA_ENV="K=V K2=V2" appends variables (values without spaces); a later PATH=... there overrides.
sbx() {
  mkdir -p "$SANDBOX_HOME/tmp" "$SANDBOX_HOME/.config" "$SANDBOX_HOME/.cache" "$SANDBOX_HOME/.local/share" "$SANDBOX_HOME/.local/state"
  # shellcheck disable=SC2086
  env -i \
    HOME="$SANDBOX_HOME" \
    PATH="$SBX_PATH" \
    TMPDIR="$SANDBOX_HOME/tmp" \
    XDG_CONFIG_HOME="$SANDBOX_HOME/.config" \
    XDG_CACHE_HOME="$SANDBOX_HOME/.cache" \
    XDG_DATA_HOME="$SANDBOX_HOME/.local/share" \
    XDG_STATE_HOME="$SANDBOX_HOME/.local/state" \
    LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8 TERM=dumb NO_COLOR=1 CI=1 \
    DO_NOT_TRACK=1 \
    CODEGRAPH_TELEMETRY=0 CODEGRAPH_NO_DAEMON=1 CODEGRAPH_NO_UPDATE_CHECK=1 \
    GORTEX_TELEMETRY=0 \
    HOMEBREW_NO_AUTO_UPDATE=1 HOMEBREW_NO_ANALYTICS=1 \
    ${SBX_EXTRA_ENV:-} \
    "$@"
}

# Absolute, symlink-free path of a directory.
abs_dir() { (cd "$1" && pwd -P); }

# Stable short key for a repo path: <basename>-<8 hex of sha256(abs path)>.
repo_key() {
  local a; a="$(abs_dir "$1")"
  printf '%s-%s' "$(basename "$a")" "$(printf '%s' "$a" | shasum -a 256 | cut -c1-8)"
}

# Apparent size in bytes of all regular files under the given paths (0 when missing).
tree_bytes() {
  local total=0 p n
  for p in "$@"; do
    [ -e "$p" ] || continue
    n=$(find "$p" -type f -exec stat -f %z {} + 2>/dev/null | awk '{s+=$1} END{printf "%d", s+0}')
    total=$((total + n))
  done
  printf '%d' "$total"
}

# Allocated size on disk in bytes (du) of the given paths.
tree_disk_bytes() {
  local total=0 p n
  for p in "$@"; do
    [ -e "$p" ] || continue
    n=$(du -sk "$p" 2>/dev/null | awk '{printf "%d", $1*1024}')
    total=$((total + ${n:-0}))
  done
  printf '%d' "$total"
}

# Throwaway APFS clone (copy-on-write, near-zero extra disk) of REPO for tools that write into the repo
# or run a write-capable daemon over it. Prints the copy's path. .git is included because several tools
# read git metadata (ignore rules, HEAD).
fresh_repo_copy() { # REPO TOOL
  local src dst
  src="$(abs_dir "$1")"; dst="$WORK_DIR/$2/copy-$(repo_key "$1")"
  rm -rf "$dst"; mkdir -p "$(dirname "$dst")"
  cp -cR "$src" "$dst" 2>/dev/null || { rm -rf "$dst"; cp -R "$src" "$dst"; }
  printf '%s' "$dst"
}
existing_repo_copy() { printf '%s' "$WORK_DIR/$2/copy-$(repo_key "$1")"; }

# _rss_sampler OUT_FILE : background loop. Every ~0.2 s it sums the RSS of every process whose command
# line matches the extended regex in $PS_MATCH, plus all of their descendants, and keeps the maximum (KiB)
# in OUT_FILE. Needed because several tools do the real work in a detached daemon or supervised worker
# (codebase-memory-mcp, gortex), where /usr/bin/time only sees a thin client.
# The pattern travels in the environment, not in argv, so the sampler never matches itself.
_rss_sampler() {
  local out="$1" max=0 cur
  echo 0 > "$out"
  while :; do
    cur=$(ps -axo pid=,ppid=,rss=,command= 2>/dev/null | PS_MATCH="$PS_MATCH" awk '
      { pid=$1; ppid=$2; rss=$3; cmd=""; for (i=4;i<=NF;i++) cmd=cmd " " $i
        par[pid]=ppid; r[pid]=rss; if (cmd ~ ENVIRON["PS_MATCH"]) root[pid]=1 }
      END {
        changed=1
        while (changed) { changed=0; for (p in par) if (!(p in root) && (par[p] in root)) { root[p]=1; changed=1 } }
        s=0; for (p in root) s+=r[p]; printf "%d", s
      }')
    if [ "${cur:-0}" -gt "$max" ]; then max=$cur; echo "$max" > "$out"; fi
    sleep 0.2
  done
}

# measure OUT_JSON TOOL REPO VERSION INDEX_PATHS_CSV -- CMD ARGS...
# Runs CMD under /usr/bin/time -l inside the sandbox, then writes one JSON object.
#   wall_seconds               : "real" from /usr/bin/time
#   peak_rss_bytes_time_l      : "maximum resident set size" from /usr/bin/time -l (bytes on macOS): the
#                                largest single process the command waited for. NOT a process-tree sum,
#                                and it does not see detached daemons.
#   peak_rss_bytes_tree_sampled: only when MEASURE_PS_MATCH (an extended regex over command lines) is set:
#                                max over ~0.2 s samples of the summed RSS of matching processes and their
#                                descendants. Sampling can miss short spikes.
#   peak_rss_bytes             : the larger of the two above (peak_rss_source says which)
#   index_bytes                : apparent bytes of the index paths after the run (comma-separated list)
#   index_disk_bytes           : allocated bytes (du) of the same paths
#   exit_status                : exit status of CMD
# MEASURE_THREADS overrides the recorded thread cap ("uncapped" when the tool has no knob).
# MEASURE_EXTRA_JSON='{"k":"v"}' is merged into the object.
# stdout+stderr of CMD go to OUT_JSON.log
measure() {
  local out="$1" tool="$2" repo="$3" version="$4" idx_csv="$5"; shift 5
  [ "$1" = "--" ] && shift
  local log="$out.log" tfile="$out.time" sfile="$out.rss" status wall rss user sys spid="" tree=""
  mkdir -p "$(dirname "$out")"
  if [ -n "${MEASURE_PS_MATCH:-}" ]; then
    PS_MATCH="$MEASURE_PS_MATCH" _rss_sampler "$sfile" & spid=$!
  fi
  sbx /usr/bin/time -l -o "$tfile" "$@" >"$log" 2>&1
  status=$?
  if [ -n "$spid" ]; then
    kill "$spid" 2>/dev/null; wait "$spid" 2>/dev/null
    tree=$(( $(cat "$sfile" 2>/dev/null || echo 0) * 1024 ))
    rm -f "$sfile"
  fi
  wall=$(awk '/ real /{print $1; exit}' "$tfile" 2>/dev/null)
  user=$(awk '/ real /{print $3; exit}' "$tfile" 2>/dev/null)
  sys=$(awk '/ real /{print $5; exit}' "$tfile" 2>/dev/null)
  rss=$(awk '/maximum resident set size/{print $1; exit}' "$tfile" 2>/dev/null)
  local IFS=','; local -a paths=($idx_csv); unset IFS
  local ib db files
  ib=$(tree_bytes "${paths[@]}"); db=$(tree_disk_bytes "${paths[@]}")
  files=$(git -C "$repo" ls-files 2>/dev/null | wc -l | tr -d ' ')
  local cmd_str; cmd_str=$(printf '%q ' "$@")
  /usr/bin/python3 -I "$(dirname "${BASH_SOURCE[0]}")/_write_json.py" \
    "$out" "$tool" "$repo" "$version" "$idx_csv" "$status" "${wall:-}" "${rss:-}" "$ib" "$db" \
    "${user:-}" "${sys:-}" "${MEASURE_THREADS:-$BENCH_JOBS}" "$files" "$cmd_str" "${tree:-}" \
    "${MEASURE_EXTRA_JSON:-}" "${SBX_EXTRA_ENV:-}"
  rm -f "$tfile"
  return "$status"
}

# Exit code used by query scripts when the tool has no such query at all.
EXIT_UNSUPPORTED=3
unsupported() { echo "UNSUPPORTED: $1" >&2; exit "$EXIT_UNSUPPORTED"; }

usage_index() { echo "usage: $(basename "$0") REPO_DIR OUT_JSON" >&2; exit 2; }
usage_query() { echo "usage: $(basename "$0") REPO_DIR def|callers|callees|locate ARG" >&2; exit 2; }

# Common argument handling.
index_args() { # "$@"  -> sets REPO OUT KEY
  [ $# -eq 2 ] || usage_index
  [ -d "$1" ] || { echo "not a directory: $1" >&2; exit 2; }
  REPO="$(abs_dir "$1")"; OUT="$2"; KEY="$(repo_key "$1")"
  case "$OUT" in /*) ;; *) OUT="$PWD/$OUT";; esac
  mkdir -p "$(dirname "$OUT")"
}
query_args() { # "$@"  -> sets REPO KIND ARG KEY
  [ $# -eq 3 ] || usage_query
  [ -d "$1" ] || { echo "not a directory: $1" >&2; exit 2; }
  REPO="$(abs_dir "$1")"; KIND="$2"; ARG="$3"; KEY="$(repo_key "$1")"
  case "$KIND" in def|callers|callees|locate) ;; *) usage_query;; esac
}

# source_file_list REPO OUT_LIST : newline-separated source files, relative to REPO, for the classical
# indexers (ctags, cscope, gtags). They choke on binaries (gtags' pygments plug-in aborts on a gettext
# .mo file) and would otherwise index docs/JSON/locale data, so all three get the same explicit list:
# tracked files (git ls-files; find when REPO is not a git checkout) filtered by extension.
# Override the filter with BENCH_SRC_EXT_REGEX (extended regex over the path).
BENCH_SRC_EXT_REGEX_DEFAULT='\.(c|h|cc|cpp|cxx|hpp|hh|hxx|inc|S|s|py|pyi|java|go|rs|js|jsx|mjs|ts|tsx|rb|php|cs|kt|swift|scala|m|mm|lua|pl|sh)$'
source_file_list() {
  local repo="$1" out="$2" re="${BENCH_SRC_EXT_REGEX:-$BENCH_SRC_EXT_REGEX_DEFAULT}"
  mkdir -p "$(dirname "$out")"
  if git -C "$repo" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    git -C "$repo" -c core.quotepath=off ls-files
  else
    (cd "$repo" && find . -type f -not -path './.git/*' | sed 's|^\./||')
  fi | grep -E "$re" | grep -v '[[:space:]"]' | LC_ALL=C sort > "$out"
}
