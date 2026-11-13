#!/usr/bin/env bash
# prepare_tree.sh - materialise a clean source tree (no .git) for one benchmark task.
#
#   prepare_tree.sh <dataset> <task-id>                 dataset: linuxflbench | locbench | contextbench | live-kbench
#   prepare_tree.sh --repo linux --commit 5.6.7         a kernel version (tag v5.6.7; tarball when older than v2.6.11)
#   prepare_tree.sh --repo linux --commit <40-hex sha>  a kernel commit
#   prepare_tree.sh --repo owner/name --commit <sha> [--url https://...git]
#
# Prints the absolute path of the tree on stdout (nothing else goes to stdout) and exits 0.
# Trees live in  $CODEFAST_BENCH_TREES/<repo>@<commit>/   (default bench/repos/_trees); "/" in a
# repo name becomes "__". An existing tree is reused unless --force is given.
# A sidecar  $CODEFAST_BENCH_TREES/.meta/<repo>@<commit>.json  records where the tree came from,
# what it was verified against, and any files lost to a case-insensitive file system.
#
# Options:  --force     rebuild even if the tree exists
#           --verify    re-verify an existing git-sourced tree against its git tree object
#           --url URL   clone URL for a non-Linux repository (default https://github.com/<repo>.git)
#
# Environment:
#   CODEFAST_BENCH_TREES   where trees go                (default <bench>/repos/_trees)
#   CODEFAST_LINUX_FULL    full-history kernel clone     (default <bench>/repos/linux-full)
#   KERNEL_TARBALL_BASE    pre-git tarball mirror        (default https://cdn.kernel.org/pub/linux/kernel)
#
# Sources of kernel trees, in the order tried:
#   1. a tag  v<version>  already in linux-full (mainline releases and -rc tags; v2.6.11 is a tree-only tag)
#   2. the same tag fetched on demand from the stable tree (5.6.7, 2.6.33.5, ... are not in torvalds/linux):
#        https://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git , then https://github.com/gregkh/linux.git
#   3. versions older than 2.6.11 (2.4.x, 2.5.x, 2.6.0 - 2.6.10) are not in git: the tarball
#        $KERNEL_TARBALL_BASE/v2.4|v2.5|v2.6/linux-<version>.tar.xz
#      is downloaded ONLY when such a tree is requested, and checked against that directory's sha256sums.asc.
#
# Non-Linux repositories: a bare cache in .gitcache/, "git fetch --depth 1 <url> <sha>" of exactly that commit.
# Nothing here runs code from a fetched repository: no hooks, no LFS smudge, no submodule fetch.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
BENCH="$(cd "$HERE/../.." && pwd)"
TREES="${CODEFAST_BENCH_TREES:-$BENCH/repos/_trees}"
LINUX_FULL="${CODEFAST_LINUX_FULL:-$BENCH/repos/linux-full}"
TARBALL_BASE="${KERNEL_TARBALL_BASE:-https://cdn.kernel.org/pub/linux/kernel}"
STABLE_URLS="https://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git https://github.com/gregkh/linux.git"

export GIT_TERMINAL_PROMPT=0
log() { printf '[prepare_tree] %s\n' "$*" >&2; }
die() { printf '[prepare_tree] ERROR: %s\n' "$*" >&2; exit 1; }
# git with nothing inherited that could run code or rewrite content, and no stored credentials
g() {
  git -c credential.helper= -c core.fsmonitor=false -c core.hooksPath=/dev/null \
      -c core.autocrlf=false -c core.symlinks=true -c core.quotepath=false -c protocol.file.allow=never \
      -c filter.lfs.smudge= -c filter.lfs.clean= -c filter.lfs.process= -c filter.lfs.required=false \
      -c gc.auto=0 "$@"
}

usage() { sed -n '2,36p' "$0" | sed 's/^# \{0,1\}//' >&2; exit 2; }

FORCE=0; VERIFY=0; REPO=""; COMMIT=""; URL=""; DATASET=""; TASK=""
while [ $# -gt 0 ]; do
  case "$1" in
    --repo) REPO="${2:-}"; shift 2 ;;
    --commit|--version) COMMIT="${2:-}"; shift 2 ;;
    --url) URL="${2:-}"; shift 2 ;;
    --force) FORCE=1; shift ;;
    --verify) VERIFY=1; shift ;;
    -h|--help) usage ;;
    --*) die "unknown option $1" ;;
    *) if [ -z "$DATASET" ]; then DATASET="$1"; elif [ -z "$TASK" ]; then TASK="$1"; else die "unexpected argument $1"; fi; shift ;;
  esac
done

if [ -n "$DATASET" ]; then
  [ -n "$TASK" ] || die "usage: prepare_tree.sh <dataset> <task-id>"
  command -v uv >/dev/null || die "uv not found"
  command -v jq >/dev/null || die "jq not found"
  case "$DATASET" in
    linuxflbench) row="$(uv run -q "$HERE/load_linuxflbench.py" --ids "$TASK")" ;;
    locbench)     row="$(uv run -q "$HERE/load_locbench.py" --subset all --ids "$TASK")" ;;
    contextbench) row="$(uv run -q "$HERE/load_contextbench.py" --subset full --ids "$TASK" --extra)" ;;
    live-kbench)  row="$(uv run -q "$HERE/load_live_kbench.py" --ids "$TASK")" ;;
    *) die "unknown dataset $DATASET (linuxflbench | locbench | contextbench | live-kbench)" ;;
  esac
  [ -n "$row" ] || die "task $TASK not found in $DATASET"
  REPO="$(printf '%s' "$row" | jq -r '.repo')"
  COMMIT="$(printf '%s' "$row" | jq -r '.commit_or_version')"
  [ -n "$URL" ] || URL="$(printf '%s' "$row" | jq -r '.extra.repo_url // empty')"
fi
[ -n "$REPO" ] && [ -n "$COMMIT" ] || usage
case "$REPO" in *..*|/*|*[!A-Za-z0-9._/-]*) die "refusing odd repo name: $REPO" ;; esac
case "$COMMIT" in *[!A-Za-z0-9._-]*|-*) die "refusing odd commit/version: $COMMIT" ;; esac

is_sha() { printf '%s' "$1" | grep -Eq '^[0-9a-f]{40}$'; }

if [ "$REPO" = "linux" ] || [ "$REPO" = "torvalds/linux" ]; then
  REPO="linux"
  if is_sha "$COMMIT"; then REF="$COMMIT"; else REF="v${COMMIT#v}"; fi
else
  is_sha "$COMMIT" || die "non-Linux repositories need a full 40-hex commit sha, got: $COMMIT"
  REF="$COMMIT"
  [ -n "$URL" ] || URL="https://github.com/$REPO.git"
fi
NAME="$(printf '%s' "$REPO" | sed 's#/#__#g')@$REF"
DEST="$TREES/$NAME"
META="$TREES/.meta/$NAME.json"
mkdir -p "$TREES/.meta" "$TREES/.locks" "$TREES/.tmp"

# ---- per-tree lock (mkdir is atomic); a second caller waits for the first to finish -------------
LOCK="$TREES/.locks/$NAME.lock"
waited=0
while ! mkdir "$LOCK" 2>/dev/null; do
  if [ -f "$LOCK/pid" ] && ! kill -0 "$(cat "$LOCK/pid" 2>/dev/null)" 2>/dev/null; then
    log "removing stale lock $LOCK"; rm -rf "$LOCK"; continue
  fi
  [ $waited -lt 7200 ] || die "timed out waiting for $LOCK"
  sleep 2; waited=$((waited + 2))
done
echo $$ > "$LOCK/pid"
TMP=""; IDX=""
cleanup() { [ -n "$TMP" ] && rm -rf "$TMP"; [ -n "$IDX" ] && rm -f "$IDX" "$IDX.lock"; rm -rf "$LOCK"; }
trap cleanup EXIT

# ---- helpers --------------------------------------------------------------------------------------
fs_case_sensitive() {  # is $TREES on a case-sensitive file system?
  local d="$TREES/.tmp/casetest.$$"; mkdir -p "$d"; : > "$d/a"
  if [ -e "$d/A" ]; then rm -rf "$d"; return 1; else rm -rf "$d"; return 0; fi
}

# verify_git_tree <git-dir> <tree-ish> <worktree> <out-prefix>
# Re-hashes every file of <worktree> against the blobs of <tree-ish>. Writes
#   <out-prefix>.diff    paths whose content/mode/type differs or is missing ("M path" / "D path" ...)
#   <out-prefix>.extra   paths on disk that the tree does not have
verify_git_tree() {
  local gd="$1" tree="$2" wt="$3" out="$4" vidx
  vidx="$(mktemp "$TREES/.tmp/vidx.XXXXXX")"; rm -f "$vidx"
  GIT_INDEX_FILE="$vidx" g --git-dir="$gd" read-tree "$tree"
  GIT_INDEX_FILE="$vidx" g --git-dir="$gd" --work-tree="$wt" update-index -q --refresh >/dev/null 2>&1 || true
  GIT_INDEX_FILE="$vidx" g --git-dir="$gd" --work-tree="$wt" diff-files --name-status > "$out.diff"
  GIT_INDEX_FILE="$vidx" g --git-dir="$gd" --work-tree="$wt" ls-files --others > "$out.extra"
  rm -f "$vidx" "$vidx.lock"
}

# materialise_git <git-dir> <tree-ish> <source-kind> <source-url> <resolved-commit-or-empty>
materialise_git() {
  local gd="$1" treeish="$2" kind="$3" src="$4" resolved="$5"
  local tree n_expected n_gitlinks collisions n_coll n_disk cs n_diff n_extra unexpected
  tree="$(g --git-dir="$gd" rev-parse --verify -q "$treeish^{tree}")" || die "cannot resolve $treeish to a tree"
  TMP="$(mktemp -d "$TREES/.tmp/$NAME.XXXXXX")"
  IDX="$(mktemp "$TREES/.tmp/idx.XXXXXX")"; rm -f "$IDX"
  GIT_INDEX_FILE="$IDX" g --git-dir="$gd" read-tree "$tree"
  log "writing files for $NAME (tree $tree)"
  GIT_INDEX_FILE="$IDX" g --git-dir="$gd" --work-tree="$TMP" checkout-index -a -f
  n_expected="$(GIT_INDEX_FILE="$IDX" g --git-dir="$gd" ls-files -s | awk '$1 != "160000"' | wc -l | tr -d ' ')"
  n_gitlinks="$(GIT_INDEX_FILE="$IDX" g --git-dir="$gd" ls-files -s | awk '$1 == "160000"' | wc -l | tr -d ' ')"
  # paths that collide when case is folded (lost on a case-insensitive file system)
  collisions="$TREES/.tmp/$NAME.collisions.$$"
  GIT_INDEX_FILE="$IDX" g --git-dir="$gd" ls-files \
    | LC_ALL=C awk '{k=tolower($0); if (k in s) { if (!(k in p)) { print s[k]; p[k]=1 } print $0 } else s[k]=$0}' > "$collisions"
  n_coll="$(wc -l < "$collisions" | tr -d ' ')"
  n_disk="$(find "$TMP" \( -type f -o -type l \) | wc -l | tr -d ' ')"
  if fs_case_sensitive; then cs=true; else cs=false; fi

  verify_git_tree "$gd" "$tree" "$TMP" "$TREES/.tmp/$NAME.verify.$$"
  n_diff="$(wc -l < "$TREES/.tmp/$NAME.verify.$$.diff" | tr -d ' ')"
  n_extra="$(wc -l < "$TREES/.tmp/$NAME.verify.$$.extra" | tr -d ' ')"
  # every differing path must be explained by a case collision or be a submodule gitlink
  unexpected="$(cut -f2- "$TREES/.tmp/$NAME.verify.$$.diff" | LC_ALL=C sort -u \
      | LC_ALL=C comm -23 - <(LC_ALL=C sort -u "$collisions") \
      | while IFS= read -r p; do
          m="$(GIT_INDEX_FILE="$IDX" g --git-dir="$gd" ls-files -s -- "$p" | awk '{print $1}')"
          [ "$m" = "160000" ] || printf '%s\n' "$p"
        done)"
  if [ -n "$unexpected" ] || [ "$n_extra" != "0" ]; then
    printf '%s\n' "$unexpected" | head -20 >&2
    head -20 "$TREES/.tmp/$NAME.verify.$$.extra" >&2
    rm -f "$collisions" "$TREES/.tmp/$NAME.verify.$$".*
    die "verification failed for $NAME: files differ from git tree $tree (see above)"
  fi
  [ "$n_coll" = "0" ] || [ "$cs" = "true" ] || \
    log "WARNING: $n_coll paths differ only by case; this file system is case-insensitive, so $(( n_expected - n_disk )) file(s) are missing from the tree (listed in $META)"

  jq -n --arg name "$NAME" --arg repo "$REPO" --arg requested "$COMMIT" --arg ref "$treeish" \
        --arg kind "$kind" --arg src "$src" --arg resolved "$resolved" --arg tree "$tree" \
        --argjson n_expected "$n_expected" --argjson n_disk "$n_disk" --argjson n_gitlinks "$n_gitlinks" \
        --argjson cs "$cs" --argjson n_diff "$n_diff" \
        --rawfile coll "$collisions" --rawfile diff "$TREES/.tmp/$NAME.verify.$$.diff" \
        --arg git "$(git --version)" --arg when "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
        '{name:$name, repo:$repo, requested:$requested, ref:$ref, source:$kind, source_url:$src,
          resolved_commit:(if $resolved=="" then null else $resolved end), git_tree:$tree,
          files_in_git_tree:$n_expected, files_on_disk:$n_disk, submodule_gitlinks_not_fetched:$n_gitlinks,
          case_sensitive_fs:$cs,
          case_colliding_paths:($coll|split("\n")|map(select(length>0))),
          paths_not_matching_git:($diff|split("\n")|map(select(length>0))),
          verification:{method:"git read-tree + update-index --refresh + diff-files against the tree object; ls-files --others",
                        unexplained_differences:0, untracked_extra_files:0},
          created_utc:$when, git_version:$git}' > "$TMP.meta.json"
  rm -f "$collisions" "$TREES/.tmp/$NAME.verify.$$".*
  rm -rf "$DEST"; mv "$TMP" "$DEST"; mv "$TMP.meta.json" "$META"; TMP=""
  log "ok: $n_disk files on disk, $n_expected in git tree, $n_coll case-colliding paths"
}

linux_gitdir() {
  [ -d "$LINUX_FULL" ] || die "kernel clone not found at $LINUX_FULL (git clone https://github.com/torvalds/linux.git $LINUX_FULL)"
  g -C "$LINUX_FULL" rev-parse --absolute-git-dir
}

# with_linux_lock <cmd...>: serialise fetches into linux-full
with_linux_lock() {
  local l="$TREES/.locks/linux-full-fetch.lock" w=0
  while ! mkdir "$l" 2>/dev/null; do [ $w -lt 3600 ] || die "timed out waiting for $l"; sleep 2; w=$((w + 2)); done
  local rc=0; "$@" || rc=$?; rm -rf "$l"; return $rc
}

pre_git_series() {  # echo v2.4 | v2.5 | v2.6 when the version predates the git tags, else nothing
  local v="${1#v}"
  case "$v" in
    2.4|2.4.*) echo v2.4 ;;
    2.5.*) echo v2.5 ;;
    2.6.[0-9]|2.6.[0-9].*|2.6.[0-9]-*|2.6.10|2.6.10.*|2.6.10-*|2.6.11.*|2.6.11-*) echo v2.6 ;;
    2.[0-3]|2.[0-3].*|1.*|0.*) echo "v${v%%.*}.$(printf '%s' "$v" | cut -d. -f2)" ;;
  esac
}

materialise_tarball() {
  local ver="${REF#v}" series="$1" tb url sums want got n_disk
  mkdir -p "$TREES/.tarballs"
  tb="$TREES/.tarballs/linux-$ver.tar.xz"
  url="$TARBALL_BASE/$series/linux-$ver.tar.xz"
  sums="$TREES/.tarballs/$series.sha256sums.asc"
  [ -s "$sums" ] || curl -fsSL -A "curl" -o "$sums" "$TARBALL_BASE/$series/sha256sums.asc" || die "cannot fetch $TARBALL_BASE/$series/sha256sums.asc"
  want="$(awk -v f="linux-$ver.tar.xz" '$2 == f {print $1}' "$sums" | head -1)"
  [ -n "$want" ] || die "linux-$ver.tar.xz is not listed in $TARBALL_BASE/$series/sha256sums.asc"
  if [ ! -s "$tb" ]; then
    log "downloading $url"
    curl -fsSL -A "curl" -o "$tb.part" "$url" || die "download failed: $url"
    mv "$tb.part" "$tb"
  fi
  got="$(shasum -a 256 "$tb" | awk '{print $1}')"
  [ "$got" = "$want" ] || { rm -f "$tb"; die "sha256 mismatch for linux-$ver.tar.xz: want $want got $got (tarball deleted)"; }
  TMP="$(mktemp -d "$TREES/.tmp/$NAME.XXXXXX")"
  # bsdtar refuses absolute paths and ".." members by default; strip the single top-level directory
  tar -xJf "$tb" -C "$TMP" --strip-components 1 || log "WARNING: tar reported errors (usually case-colliding names)"
  n_disk="$(find "$TMP" \( -type f -o -type l \) | wc -l | tr -d ' ')"
  local n_members n_coll cs
  n_members="$(tar -tJf "$tb" | grep -vc '/$' || true)"
  local collisions="$TREES/.tmp/$NAME.collisions.$$"
  tar -tJf "$tb" | grep -v '/$' | sed 's#^[^/]*/##' \
    | LC_ALL=C awk '{k=tolower($0); if (k in s) { if (!(k in p)) { print s[k]; p[k]=1 } print $0 } else s[k]=$0}' > "$collisions"
  n_coll="$(wc -l < "$collisions" | tr -d ' ')"
  if fs_case_sensitive; then cs=true; else cs=false; fi
  [ "$n_disk" -gt 1000 ] || die "tarball extraction produced only $n_disk files"
  jq -n --arg name "$NAME" --arg repo "$REPO" --arg requested "$COMMIT" --arg url "$url" --arg sha "$got" \
        --arg sums "$TARBALL_BASE/$series/sha256sums.asc" --argjson n_members "$n_members" --argjson n_disk "$n_disk" \
        --rawfile coll "$collisions" --argjson cs "$cs" --arg when "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
        '{name:$name, repo:$repo, requested:$requested, source:"kernel.org tarball", source_url:$url,
          tarball_sha256:$sha, sha256_checked_against:$sums, pgp_signature_checked:false,
          file_members_in_tarball:$n_members, files_on_disk:$n_disk, case_sensitive_fs:$cs,
          case_colliding_paths:($coll|split("\n")|map(select(length>0))),
          verification:{method:"sha256 of the tarball against kernel.org sha256sums.asc; member count vs files on disk"},
          created_utc:$when}' > "$META.tmp"
  rm -f "$collisions"
  if [ "$cs" = "true" ]; then
    [ "$n_disk" = "$n_members" ] || die "extracted $n_disk files but the tarball has $n_members file members"
  else
    # each group of names that differ only by case keeps exactly one file
    [ $(( n_members - n_disk )) -le "$n_coll" ] || die "extracted $n_disk of $n_members members; more are missing than the $n_coll case-colliding names explain"
    [ "$n_coll" = "0" ] || log "WARNING: $n_coll member names differ only by case; this file system is case-insensitive, so $(( n_members - n_disk )) file(s) are missing from the tree (listed in $META)"
  fi
  rm -rf "$DEST"; mv "$TMP" "$DEST"; TMP=""; mv "$META.tmp" "$META"
  log "ok: $n_disk files on disk, $n_members file members in tarball, $n_coll case-colliding paths"
}

# ---- reuse ----------------------------------------------------------------------------------------
if [ -d "$DEST" ] && [ -s "$META" ] && [ $FORCE -eq 0 ]; then
  if [ $VERIFY -eq 1 ]; then
    tree="$(jq -r '.git_tree // empty' "$META")"
    if [ -n "$tree" ]; then
      if [ "$REPO" = "linux" ]; then gd="$(linux_gitdir)"; else gd="$TREES/.gitcache/$(printf '%s' "$REPO" | sed 's#/#__#g').git"; fi
      verify_git_tree "$gd" "$tree" "$DEST" "$TREES/.tmp/$NAME.reverify.$$"
      bad="$(cut -f2- "$TREES/.tmp/$NAME.reverify.$$.diff" | LC_ALL=C sort -u | LC_ALL=C comm -23 - <(jq -r '(.case_colliding_paths + (.paths_not_matching_git|map(split("\t")[1:]|join("\t"))))[]' "$META" | LC_ALL=C sort -u))"
      extra="$(cat "$TREES/.tmp/$NAME.reverify.$$.extra")"
      rm -f "$TREES/.tmp/$NAME.reverify.$$".*
      [ -z "$bad" ] && [ -z "$extra" ] || { printf '%s\n%s\n' "$bad" "$extra" | head -20 >&2; die "existing tree $DEST no longer matches git tree $tree (use --force to rebuild)"; }
      log "verified existing tree against $tree"
    else
      log "existing tree came from a tarball; nothing to re-verify against"
    fi
  fi
  log "reusing $DEST"
  printf '%s\n' "$DEST"; exit 0
fi
[ ! -e "$DEST" ] || { log "removing incomplete or forced tree $DEST"; rm -rf "$DEST" "$META"; }

# ---- build ----------------------------------------------------------------------------------------
if [ "$REPO" = "linux" ]; then
  if is_sha "$REF"; then
    GD="$(linux_gitdir)"
    if ! g --git-dir="$GD" cat-file -e "$REF^{commit}" 2>/dev/null; then
      log "commit $REF not in linux-full; fetching it"
      fetched=0
      for u in https://github.com/torvalds/linux.git $STABLE_URLS ${URL:+"$URL"}; do
        if with_linux_lock g --git-dir="$GD" fetch -q --no-tags "$u" "$REF" 2>/dev/null; then fetched=1; break; fi
      done
      [ $fetched -eq 1 ] && g --git-dir="$GD" cat-file -e "$REF^{commit}" 2>/dev/null \
        || die "commit $REF is not in torvalds/linux or the stable tree; pass --url <git url of the tree that has it> (linux-next, net-next, ...)"
    fi
    materialise_git "$GD" "$REF" "git commit in linux-full" "https://github.com/torvalds/linux.git" "$REF"
  else
    series="$(pre_git_series "$REF")"
    if [ -n "$series" ]; then
      materialise_tarball "$series"
    else
      GD="$(linux_gitdir)"
      src="https://github.com/torvalds/linux.git"
      if ! g --git-dir="$GD" rev-parse --verify -q "refs/tags/$REF^{tree}" >/dev/null; then
        log "tag $REF not in linux-full; fetching it from the stable tree"
        ok=0
        for u in $STABLE_URLS; do
          if with_linux_lock g --git-dir="$GD" fetch -q --no-tags "$u" "refs/tags/$REF:refs/tags/$REF"; then ok=1; src="$u"; break; fi
          log "fetch of $REF from $u failed"
        done
        [ $ok -eq 1 ] || die "tag $REF not found in torvalds/linux or the stable tree"
      fi
      # stable point releases (X.Y.Z from 3.0 on, 2.6.X.Y before) are never in torvalds/linux
      case "${REF#v}" in
        *-rc*) : ;;
        2.6.*.*|[3-9].*.*|[1-9][0-9].*.*) src="${STABLE_URLS%% *}" ;;
      esac
      resolved="$(g --git-dir="$GD" rev-parse --verify -q "refs/tags/$REF^{commit}" 2>/dev/null || true)"
      materialise_git "$GD" "refs/tags/$REF" "git tag in linux-full" "$src" "$resolved"
    fi
  fi
else
  GD="$TREES/.gitcache/$(printf '%s' "$REPO" | sed 's#/#__#g').git"
  if [ ! -d "$GD" ]; then
    mkdir -p "$TREES/.gitcache"
    g init -q --bare "$GD"
  fi
  if ! g --git-dir="$GD" cat-file -e "$REF^{commit}" 2>/dev/null; then
    log "shallow-fetching $REF from $URL"
    g --git-dir="$GD" fetch -q --depth 1 --no-tags "$URL" "$REF" || die "fetch of $REF from $URL failed"
    g --git-dir="$GD" cat-file -e "$REF^{commit}" 2>/dev/null || die "fetched, but $REF is still missing"
  fi
  materialise_git "$GD" "$REF" "shallow fetch of one commit" "$URL" "$REF"
fi

[ -d "$DEST" ] && [ -s "$META" ] || die "internal error: $DEST or $META missing after build"
printf '%s\n' "$DEST"
