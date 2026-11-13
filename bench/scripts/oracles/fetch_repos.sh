#!/bin/bash
# Clone the oracle target repositories at their pinned commits into
# bench/repos/. Pins are the ones repowise-bench records (graph/corpus/corpus.lock
# for gitleaks, syft and zod; graph/experiments/g9-language-oracles/README.md
# for redis 8.10.2).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPOS="$(cd "$HERE/../.." && pwd)/repos"
mkdir -p "$REPOS"

fetch() {
  local name="$1" url="$2" sha="$3" dir="$REPOS/$1"
  if [ -d "$dir/.git" ]; then
    local have; have="$(git -C "$dir" rev-parse HEAD)"
    if [ "$have" = "$sha" ]; then echo "$name: already at $sha"; return; fi
    echo "$name: exists at $have, expected $sha -- not touching it" >&2; return 1
  fi
  mkdir "$dir"
  git -C "$dir" init -q
  git -C "$dir" remote add origin "$url"
  git -C "$dir" fetch -q --depth 1 origin "$sha"
  git -C "$dir" -c advice.detachedHead=false checkout -q FETCH_HEAD
  echo "$name: $(git -C "$dir" rev-parse HEAD)"
}

fetch gitleaks https://github.com/gitleaks/gitleaks.git 8ad8470035d31a209322c580153b45c18e21b980
fetch syft     https://github.com/anchore/syft.git      b15c5dbfe2bb21c9d73002c1056a829c8c411c75
fetch zod      https://github.com/colinhacks/zod.git    bbc68f990c7e6a5e3f506c56fb04bd0279b9c9b5
fetch redis    https://github.com/redis/redis.git       498ecd0d6d007db11ddb3aea9428552598a78622
# Not one of repowise-bench's pins: the head of go-git on 2026-11-13, added as a repository
# that nothing in codefast had been run on, for the held-out tasks of bench/tier2.
fetch go-git   https://github.com/go-git/go-git.git     12b788e49978b5807b748e425de6fba31af979c9
