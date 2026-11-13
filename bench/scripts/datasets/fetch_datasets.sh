#!/usr/bin/env bash
# fetch_datasets.sh - re-download every benchmark dataset at the exact revisions recorded on 2026-11-12.
#
#   fetch_datasets.sh [linuxflbench] [locbench] [contextbench] [live-kbench]      (default: all four)
#
# Writes under $CODEFAST_BENCH_DATA (default <bench>/data), one directory per source, skipping anything
# already present, then checks  <bench>/data/SHA256SUMS  for the datasets it fetched.
# Everything is anonymous: no gh, no tokens, no stored git credentials, a generic User-Agent.
# Nothing downloaded is executed. Requires git, curl, shasum, uv (only for the live-kbench extract step).
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
BENCH="$(cd "$HERE/../.." && pwd)"
DATA="${CODEFAST_BENCH_DATA:-$BENCH/data}"
SUMS="$BENCH/data/SHA256SUMS"
export GIT_TERMINAL_PROMPT=0

log() { printf '[fetch] %s\n' "$*" >&2; }
die() { printf '[fetch] ERROR: %s\n' "$*" >&2; exit 1; }
g() { git -c credential.helper= -c core.fsmonitor=false -c core.hooksPath=/dev/null -c advice.detachedHead=false "$@"; }

# pin_clone <url> <commit> <dest> [sparse pattern ...]
# Shallow-fetches exactly <commit>; with patterns, a blob-less sparse checkout of only those paths.
pin_clone() {
  local url="$1" sha="$2" dest="$3"; shift 3
  if [ -d "$dest/.git" ] && [ "$(g -C "$dest" rev-parse HEAD 2>/dev/null)" = "$sha" ]; then log "have $dest @ $sha"; return 0; fi
  [ ! -e "$dest" ] || die "$dest exists but is not at $sha; move it away first"
  mkdir -p "$dest"; g -C "$dest" init -q
  g -C "$dest" remote add origin "$url"
  if [ $# -gt 0 ]; then
    g -C "$dest" config remote.origin.promisor true
    g -C "$dest" config remote.origin.partialclonefilter blob:none
    g -C "$dest" sparse-checkout set --no-cone "$@"
    g -C "$dest" fetch -q --depth 1 --filter=blob:none --no-tags origin "$sha"
  else
    g -C "$dest" fetch -q --depth 1 --no-tags origin "$sha"
  fi
  g -C "$dest" checkout -q FETCH_HEAD
  [ "$(g -C "$dest" rev-parse HEAD)" = "$sha" ] || die "$dest: HEAD is not $sha"
  log "fetched $url @ $sha -> $dest"
}

# hf_file <dataset repo> <revision> <path in repo> <dest dir>
hf_file() {
  local repo="$1" rev="$2" path="$3" dest="$4/$3"
  [ -s "$dest" ] && { log "have $dest"; return 0; }
  mkdir -p "$(dirname "$dest")"
  curl -fsSL -A "curl" -o "$dest.part" "https://huggingface.co/datasets/$repo/resolve/$rev/$path" || die "download failed: $repo/$path"
  mv "$dest.part" "$dest"; log "fetched hf:$repo@$rev/$path"
}

fetch_linuxflbench() {
  pin_clone https://github.com/FudanSELab/LinuxFLBench.git de65c3958f264137835eb23cacf3591fb89fc09b "$DATA/linuxflbench/upstream"
}

fetch_locbench() {
  local d="$DATA/locbench"
  hf_file czlll/Loc-Bench_V1 c44cf3b74e07ca642cec841b471a9939907c12a7 README.md "$d/hf-czlll-Loc-Bench_V1"
  hf_file czlll/Loc-Bench_V1 c44cf3b74e07ca642cec841b471a9939907c12a7 data/test-00000-of-00001.parquet "$d/hf-czlll-Loc-Bench_V1"
  pin_clone https://github.com/redhat-et/ripwire.git c4a991c238dbc5319c076eb39b9624e0a5db8414 "$d/ripwire" \
            '/bench/headtohead/' '/bench/locbench/' '/LICENSE' '/NOTICE'
  pin_clone https://github.com/gersteinlab/LocAgent.git 4935b557326c154bad8e8dcf3747cc8d32d1f387 "$d/locagent" \
            '/evaluation/eval_metric.py' '/LICENSE' '/util/benchmark/' '/README.md'
  # ripwire's frozen dataset snapshot: the 560 rows as served by the HF datasets-server JSON API,
  # paged 100 at a time and re-serialised with json.dumps() exactly as bench/locbench/run_locbench.py does.
  local rows="$d/ripwire-r4-slice/rows_czlll__Loc-Bench_V1_test_560.json"
  if [ ! -s "$rows" ]; then
    mkdir -p "$(dirname "$rows")"
    python3 -I - "$rows" <<'PY'
import json, sys, time, urllib.request, urllib.parse
api, ds, n = "https://datasets-server.huggingface.co/rows", "czlll/Loc-Bench_V1", 560
rows, off = [], 0
while len(rows) < n:
    url = f"{api}?dataset={urllib.parse.quote(ds)}&config=default&split=test&offset={off}&length={min(100, n - len(rows))}"
    page = None
    for attempt in range(5):
        try:
            with urllib.request.urlopen(urllib.request.Request(url, headers={"User-Agent": "python-urllib"}), timeout=120) as r:
                page = json.load(r)
            break
        except Exception as e:
            print(f"retry {attempt + 1}/5: {e}", file=sys.stderr); time.sleep(3 * (attempt + 1))
    if page is None:
        raise SystemExit(f"datasets-server fetch failed: {url}")
    batch = [x["row"] for x in page.get("rows", [])]
    if not batch:
        break
    rows.extend(batch); off += len(batch)
open(sys.argv[1], "w").write(json.dumps(rows[:n]))
PY
    log "fetched datasets-server rows -> $rows"
  fi
}

fetch_contextbench() {
  local d="$DATA/contextbench"
  pin_clone https://github.com/EuniAI/ContextBench.git 1436c28a8eb95496da4ea69ad458b9f8a8eb7d61 "$d/upstream"
  pin_clone https://github.com/repowise-dev/repowise-bench.git 0fb3c03aab35fb67dc72540fd6a70e2c2c599878 "$d/repowise-bench"
  pin_clone https://github.com/repowise-dev/repowise.git 8bed062714b896a8d50e9567d101152654339515 "$d/repowise" \
            '/docs/BENCHMARKS.md' '/LICENSE'
}

fetch_live_kbench() {
  local d="$DATA/live-kbench" rev=e42003c93fe2c7da335d18252cb11823931445f6 f
  for f in README.md datasets/train-00000-of-00001.parquet bugs/train-00000-of-00002.parquet bugs/train-00001-of-00002.parquet; do
    hf_file live-kbench/live-kbench "$rev" "$f" "$d/hf-live-kbench"
  done
  pin_clone https://github.com/ARiSE-Lab/live-kbench.git c9d1e6cd6cf9b87d2a37eb1189ad72cfe0753514 "$d/upstream-code"
  [ -s "$d/lkbench-2512.metadata.jsonl" ] || \
    uv run -q "$HERE/extract_live_kbench.py" --src "$d/hf-live-kbench" --out "$d/lkbench-2512.metadata.jsonl"
}

check_sums() {  # check the SHA256SUMS lines that belong to one dataset directory
  local ds="$1" tmp
  [ -s "$SUMS" ] || { log "no $SUMS; skipping checksum check"; return 0; }
  tmp="$(mktemp)"; grep -E "  $ds/" "$SUMS" > "$tmp" || true
  if [ -s "$tmp" ]; then (cd "$DATA" && shasum -a 256 -c "$tmp" --quiet) || { rm -f "$tmp"; die "checksum mismatch in $ds"; }; fi
  log "$ds: $(wc -l < "$tmp" | tr -d ' ') checksums ok"; rm -f "$tmp"
}

[ $# -gt 0 ] || set -- linuxflbench locbench contextbench live-kbench
mkdir -p "$DATA"
for ds in "$@"; do
  case "$ds" in
    linuxflbench) fetch_linuxflbench ;;
    locbench) fetch_locbench ;;
    contextbench) fetch_contextbench ;;
    live-kbench) fetch_live_kbench ;;
    *) die "unknown dataset $ds" ;;
  esac
  check_sums "$ds"
done
