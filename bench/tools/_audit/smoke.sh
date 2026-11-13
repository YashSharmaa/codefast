#!/bin/bash
# smoke.sh TOOL [SYMBOL] [LOCATE_TEXT] : index django with one tool and run the four queries, capturing outputs in bench/tools/_smoke
BENCH="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
D=$BENCH/scripts/competitors
R=${SMOKE_REPO:-$BENCH/repos/django}; O=$BENCH/tools/_smoke; mkdir -p $O
t=$1; sym=${2:-get_object_or_404}; loc=${3:-where is the CSRF token validated}
if [ -z "${SMOKE_SKIP_INDEX:-}" ]; then
  $D/index_$t.sh $R $O/$t.index.json; echo "index exit=$?"
  /usr/bin/python3 -I -c 'import json,sys; d=json.load(open(sys.argv[1])); print({k:d.get(k) for k in ("wall_seconds","user_seconds","peak_rss_bytes","peak_rss_source","index_bytes","exit_status","threads_requested")})' $O/$t.index.json
  tail -c 400 $O/$t.index.json.log; echo
fi
for k in def callers callees locate; do
  a="$sym"; [ $k = locate ] && a="$loc"
  /usr/bin/time -p $D/query_$t.sh $R $k "$a" > $O/$t.$k.txt 2> $O/$t.$k.err; rc=$?
  echo "$k exit=$rc bytes=$(wc -c < $O/$t.$k.txt | tr -d ' ') lines=$(wc -l < $O/$t.$k.txt | tr -d ' ') $(grep '^real' $O/$t.$k.err) $(grep -v '^real\|^user\|^sys' $O/$t.$k.err | head -2 | cut -c1-200)"
done
echo "repo dirty lines: $(git -C $R status --porcelain --ignored | wc -l | tr -d ' ')"
